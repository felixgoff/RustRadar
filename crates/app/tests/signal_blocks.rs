//! Item T5's acceptance shot: signal blocks from a backend fixture (`--fixture`'s
//! `train_blocks.json`, a `TrainBlocks` answer as the backend's `train_blocks` sends it) drawn by
//! the whole app (`--screenshot`: the block state, the trains' and the blocks' scene builders, the
//! map) with the rail map showing the blocks, over the committed tiles.
//!
//! The place is the Arlanda line's northern portal (59.672° N, 17.92° E), the track as
//! OpenFreeMap's tile 14/9006–9007/4787 has it: two tracks about 5 m apart running east-west
//! that dive into the tunnel under the airport. Three blocks on each track, with two trains of
//! `trains_in_view.json`: one in the north track's middle block, one in the south track's. So, at
//! zoom 15.5 straight down: the casing under every block; the taken blocks red and 5 px wide, the
//! free ones green and 3 px; the tunnels faint and dashed; the signals' discs red where their block
//! is taken, yellow where the next one is, green elsewhere, with their names; the trains' names in
//! red boxes on the taken blocks. There is no reference picture with blocks (today's needs a
//! Trafikverket key); the shot without the blocks file is the comparison.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/t5-*.png`.

mod common;

use std::path::{Path, PathBuf};

use common::shots::{gpu, save};
use common::{NOW_MS, app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::state::AppState;
use rustradar_core::geo::Basemap;
use rustradar_core::model::{Structure, Train, TrainBlock, TrainBlocks, TrainKind, TrainSource};
use rustradar_core::settings::{Mode, RailShows};
use rustradar_map::headless::Image;
use rustradar_scene::blocks::{Aspect, BLOCK_CASING, BLOCK_FREE, BLOCK_LABEL_BACKGROUND, BLOCK_TAKEN};
use rustradar_scene::camera::{ViewState, viewport_for};

const SIZE: [u32; 2] = [1440, 900];
const VIEW: ViewState = ViewState { longitude: 17.9118, latitude: 59.6730, zoom: 15.5, pitch: 0.0, bearing: 0.0, position: None };

/// The north track, west to east, into the tunnel (OpenFreeMap's lines joined at the tiles' edge).
const NORTH: [[f64; 2]; 24] = [
    [17.885399, 59.674556],
    [17.887319, 59.674702],
    [17.889583, 59.674827],
    [17.891257, 59.674881],
    [17.892952, 59.674924],
    [17.894851, 59.674941],
    [17.896433, 59.674924],
    [17.898091, 59.674897],
    [17.89962, 59.674849],
    [17.901197, 59.674789],
    [17.902924, 59.674694],
    [17.905564, 59.67448],
    [17.906953, 59.674353],
    [17.908965, 59.674136],
    [17.911239, 59.673846],
    [17.913428, 59.673516],
    [17.915488, 59.673145],
    [17.91729, 59.672747],
    [17.917816, 59.672608],
    [17.91934, 59.672183],
    [17.919849, 59.672029],
    [17.920702, 59.671755],
    [17.922129, 59.671216],
    [17.922912, 59.670891],
];

/// The south track, east to west, out of the tunnel.
const SOUTH: [[f64; 2]; 22] = [
    [17.922912, 59.670794],
    [17.92222, 59.671105],
    [17.920584, 59.671736],
    [17.919812, 59.671985],
    [17.919431, 59.672102],
    [17.918293, 59.672432],
    [17.917773, 59.672576],
    [17.917247, 59.672706],
    [17.915483, 59.673107],
    [17.913385, 59.673483],
    [17.911196, 59.673811],
    [17.908965, 59.674098],
    [17.906878, 59.67431],
    [17.905537, 59.674437],
    [17.904035, 59.674551],
    [17.902919, 59.674643],
    [17.901288, 59.674727],
    [17.89962, 59.6748],
    [17.89807, 59.674849],
    [17.89645, 59.674873],
    [17.894835, 59.674889],
    [17.892941, 59.674873],
];

/// The tunnel under the airport: each track's line from the portal on.
const TUNNEL_NORTH: [[f64; 2]; 4] = [[17.920702, 59.671755], [17.922129, 59.671216], [17.922912, 59.670891], [17.923717, 59.670504]];
const TUNNEL_SOUTH: [[f64; 2]; 4] = [[17.923781, 59.670387], [17.922912, 59.670794], [17.92222, 59.671105], [17.920584, 59.671736]];

/// The part of `track` between the longitudes `from` and `to` (in the track's direction), cut where
/// it crosses them.
fn cut(track: &[[f64; 2]], from: f64, to: f64) -> Vec<[f64; 2]> {
    let (lo, hi) = (from.min(to), from.max(to));
    let at = |a: [f64; 2], b: [f64; 2], lon: f64| {
        let t = (lon - a[0]) / (b[0] - a[0]);
        [lon, a[1] + (b[1] - a[1]) * t]
    };
    let mut out = Vec::new();
    for pair in track.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let inside = |p: [f64; 2]| p[0] >= lo && p[0] <= hi;
        if inside(a) && out.is_empty() {
            out.push(a);
        }
        for lon in [from, to] {
            if (a[0] - lon) * (b[0] - lon) < 0.0 {
                out.push(at(a, b, lon));
            }
        }
        if inside(b) {
            out.push(b);
        }
    }
    out
}

fn block(id: &str, signal: &str, line: Vec<[f64; 2]>) -> TrainBlock {
    TrainBlock { id: id.into(), signal: Some(signal.into()), lines: vec![line] }
}

/// The blocks: north `n1` (free, its next block taken), `n2` (taken), `n3` (free, on into the
/// tunnel); south `s1` (out of the tunnel, free, its next taken), `s2` (taken), `s3` (free).
fn blocks() -> TrainBlocks {
    TrainBlocks {
        blocks: vec![
            block("n1", "61", cut(&NORTH, 17.9020, 17.9105)),
            block("n2", "63", cut(&NORTH, 17.9105, 17.9160)),
            block("n3", "65", cut(&NORTH, 17.9160, 17.9229)),
            block("s1", "62", cut(&SOUTH, 17.9229, 17.9155)),
            block("s2", "64", cut(&SOUTH, 17.9155, 17.9030)),
            block("s3", "66", cut(&SOUTH, 17.9030, 17.8930)),
        ],
        structures: vec![
            Structure { tunnel: true, line: TUNNEL_NORTH.to_vec(), joined: [false, true] },
            Structure { tunnel: true, line: TUNNEL_SOUTH.to_vec(), joined: [true, false] },
        ],
        complete: true,
    }
}

fn train(n: u32, name: &str, [lon, lat]: [f64; 2], heading: f32) -> Train {
    Train {
        id: format!("trafikverket:2026-06-21:{n}"),
        source: TrainSource::Trafikverket,
        name: name.into(),
        kind: TrainKind::Regional,
        lat,
        lon,
        heading: Some(heading),
        speed_kmh: Some(140.0),
        destination: None,
        operator: None,
        delay_minutes: None,
        position_unix: (NOW_MS / 1000.0) as i64,
        frames: Vec::new(),
        reports: Vec::new(),
        predicted: Vec::new(),
        cars: None,
        tail: Vec::new(),
        structures: Vec::new(),
        ended_unix: None,
        shape: None,
        model: None,
    }
}

/// The trains: one on each track, in its middle block.
fn trains() -> Vec<Train> {
    vec![train(7041, "Arlanda Express 7041", NORTH[15], 110.0), train(7046, "Arlanda Express 7046", SOUTH[11], 290.0)]
}

/// A `--fixture` folder `name` with the trains, the blocks when `with_blocks`, and the committed
/// tiles linked in.
fn folder(name: &str, with_blocks: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("the fixture folder");
    std::fs::write(dir.join("trains_in_view.json"), serde_json::to_vec(&trains()).unwrap()).unwrap();
    let file = dir.join("train_blocks.json");
    if with_blocks {
        std::fs::write(&file, serde_json::to_vec(&blocks()).unwrap()).unwrap();
    } else if file.exists() {
        std::fs::remove_file(&file).unwrap();
    }
    link_tiles(&dir);
    dir
}

#[cfg(unix)]
fn link_tiles(dir: &Path) {
    let link = dir.join("tiles");
    if std::fs::symlink_metadata(&link).is_err() {
        std::os::unix::fs::symlink(rustradar_map::io::fixture_tiles_dir(), &link).expect("the tiles linked in");
    }
}

/// Elsewhere the shots draw on the bare land: the comparisons hold all the same.
#[cfg(not(unix))]
fn link_tiles(_dir: &Path) {}

/// The rail map showing the blocks, trains on the map.
fn blocks_on(state: &mut AppState) {
    let settings = &mut state.persisted.settings;
    settings.rail_map = true;
    settings.rail_shows = RailShows::Blocks;
    settings.mode = Mode::Trains;
}

fn screen(p: [f64; 2]) -> [f64; 2] {
    let at = viewport_for(&VIEW, SIZE.map(f64::from)).project([p[0], p[1], 0.0]);
    [at[0], at[1]]
}

/// `top` at its alpha over `under`.
fn over(top: [u8; 4], under: [u8; 4]) -> [u8; 4] {
    let a = f64::from(top[3]) / 255.0;
    let c = |i: usize| (f64::from(top[i]) * a + f64::from(under[i]) * (1.0 - a)).round() as u8;
    [c(0), c(1), c(2), 255]
}

fn near(p: [u8; 4], want: [u8; 4], tolerance: u8) -> bool {
    (0..3).all(|c| p[c].abs_diff(want[c]) <= tolerance)
}

/// The pixels down a column of 24 through `[x, y]`.
fn column(image: &Image, [x, y]: [f64; 2]) -> Vec<(u32, [u8; 4])> {
    ((y - 12.0) as u32..(y + 12.0) as u32).map(|py| (py, image.pixel(x as u32, py))).collect()
}

#[test]
fn signal_blocks_from_a_backend_fixture_over_the_rail_map() {
    let Some(gpu) = gpu() else { return };
    let with_blocks = Options { fixture: Some(folder("t5-blocks", true)), ..options(VIEW, Basemap::Dark, SIZE, 1.0) };
    let without = Options { fixture: Some(folder("t5-no-blocks", false)), ..options(VIEW, Basemap::Dark, SIZE, 1.0) };
    let image = app_shot_with(&gpu, &with_blocks, blocks_on);
    let plain = app_shot_with(&gpu, &without, blocks_on);
    save(&image, "t5-blocks-arlanda-1440x900.png");
    save(&plain, "t5-no-blocks-arlanda-1440x900.png");

    // across the two tracks (6 px apart) where both blocks are free (n3 and s1) and where both are
    // taken (n2 and s2): each block's colour over its casing, 3 px or 5 px wide, the casing over the
    // map outside them
    for (lon, color, width) in [(17.9185, BLOCK_FREE, 3), (17.9118, BLOCK_TAKEN, 5)] {
        let north = cut(&NORTH, lon - 0.0002, lon + 0.0002);
        let south = cut(&SOUTH, lon + 0.0002, lon - 0.0002);
        let mid = [lon, (north[0][1] + north[north.len() - 1][1] + south[0][1] + south[south.len() - 1][1]) / 4.0];
        let at = screen(mid);
        let col = column(&image, at);
        let base = column(&plain, at);
        let full = col.iter().zip(&base).filter(|((_, p), (_, under))| near(*p, over(color, over(BLOCK_CASING, *under)), 6)).count();
        let casing = col.iter().zip(&base).filter(|((_, p), (_, under))| near(*p, over(BLOCK_CASING, *under), 6) && p != under).count();
        assert!(full + 2 >= 2 * width && full <= 2 * width + 2, "{color:?} at {lon}: {full} px across two tracks, want 2 × {width}: {col:?}");
        assert!(casing >= 2, "{color:?} at {lon}: {casing} px of casing: {col:?}");
    }

    // the signals: n1 caution (n2 taken), n2 stop, n3 clear; s2 stop, s3 clear (s1's is beyond the
    // right edge)
    for (at, aspect) in [
        (blocks().blocks[0].lines[0][0], Aspect::Caution),
        (blocks().blocks[1].lines[0][0], Aspect::Stop),
        (blocks().blocks[2].lines[0][0], Aspect::Clear),
        (blocks().blocks[4].lines[0][0], Aspect::Stop),
        (blocks().blocks[5].lines[0][0], Aspect::Clear),
    ] {
        let [x, y] = screen(at);
        let p = image.pixel(x as u32, y as u32);
        assert!(near(p, aspect.color(), 4), "{aspect:?} at {at:?}: {p:?}");
    }

    // the tunnel east of the portal: faint dashes, the map between them
    let portal = screen([17.9212, 59.671567]);
    let deep = screen([17.9221, 59.671227]);
    let steps: u32 = 60;
    let mut marks = Vec::new();
    for k in 0..=steps {
        let t = f64::from(k) / f64::from(steps);
        let [x, y] = [portal[0] + (deep[0] - portal[0]) * t, portal[1] + (deep[1] - portal[1]) * t];
        let (p, under) = (image.pixel(x as u32, y as u32), plain.pixel(x as u32, y as u32));
        marks.push(p[1] > under[1] + 20);
    }
    let lit = marks.iter().filter(|&&m| m).count() as u32;
    assert!(lit > steps / 5 && lit < steps * 4 / 5, "dashed in the tunnel: {lit} of {steps}: {marks:?}");

    // each train's name in a red box halfway along its block
    let label_box = BLOCK_LABEL_BACKGROUND;
    for (k, name) in [(1, "Arlanda Express 7041"), (4, "Arlanda Express 7046")] {
        let line = &blocks().blocks[k].lines[0];
        let [x, y] = screen(line[line.len() / 2]);
        let mut boxed = 0;
        for dy in -5..=5 {
            for dx in -60..=60 {
                let (px, py) = ((x + f64::from(dx)) as u32, (y + f64::from(dy)) as u32);
                if near(image.pixel(px, py), over(label_box, plain.pixel(px, py)), 12) {
                    boxed += 1;
                }
            }
        }
        assert!(boxed > 400, "{name}'s box: {boxed} px");
    }

    // nothing of the blocks far from the track
    let far = screen([17.9118, 59.6705]);
    assert_eq!(image.pixel(far[0] as u32, far[1] as u32), plain.pixel(far[0] as u32, far[1] as u32));
}
