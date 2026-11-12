//! The native RustRadar app: a winit window, the map of `rustradar-map`, an egui UI, the backend
//! of `rustradar-backend` on a tokio runtime (plan 1.1, 2).
//!
//! - [`app`]: the window and the frame loop of plan 2.2 ([`app::Engine`] draws a frame; the
//!   window and the headless path share it).
//! - [`state`]: the state machine, one file per feed; [`scene_builder`]: the state turned into the
//!   map's [`rustradar_map::SceneInput`] at each scene refresh.
//! - [`requests`]: the runtime, the wake, [`requests::Pending`]. [`input`]: events between egui
//!   and the map. [`platform`]: folders and other platform glue.
//! - `--screenshot` is drawn by [`headless`].
//!
//! Entry points: the desktop binary (`main.rs`), `android_main` (Android's activity calls it on its
//! own thread) and `rustradar_ios_main` (iOS's `main.m` calls it).

pub mod app;
pub mod assets;
pub mod dev;
pub mod headless;
pub mod images;
pub mod input;
pub mod persist;
pub mod platform;
pub mod requests;
pub mod scene_builder;
pub mod state;
pub mod tile_fetcher;
pub mod ui;
pub mod update;

use std::path::PathBuf;

use rustradar_core::geo::Basemap;
use rustradar_scene::camera::ViewState;

use crate::state::Startup;

/// The command line (plan section 8).
pub const USAGE: &str = "usage: rustradar-app [--view LAT,LON,ZOOM[,PITCH[,BEARING]]] [--basemap dark|satellite|realistic]
                     [--size WIDTHxHEIGHT] [--now UNIX_MS] [--fixture DIR] [--demo]
                     [--screenshot FILE.png [--scale FACTOR] [--wait SECONDS] [--ui] [--select ID]
                     [--mode aircraft|trains|ships]] [--updated]

  --view        open the map there (degrees; zoom as deck.gl's; pitch and bearing default to 0)
  --basemap     the basemap to show instead of the saved one
  --size        the window's inner size, or the shot's canvas (default 1100x720), in CSS pixels
  --now         the local clock starts at this time (Unix milliseconds); a shot's clock stands still there
  --fixture     canned feeds from this folder instead of the network
  --demo        the demo trains
  --screenshot  draw one frame without a window into this PNG, and exit
  --scale       the shot's device pixels per CSS pixel (default 1)
  --wait        how long a shot waits for the map's tiles before it is taken anyway (default
                $RUSTRADAR_SHOT_WAIT, else 30 s in a release build, 60 s in a debug build)
  --ui          the shot draws the UI over the map as the window does (default: the map alone)
  --select      the shot opens with this flight selected (its id as the fixture's files name it)
  --mode        the shot's focus, as if saved in the settings (default: aircraft)
  --updated     started by the updater after an install: no update check at launch";

/// The canvas of a shot without `--size`, CSS pixels.
pub const SHOT_SIZE: [u32; 2] = [1100, 720];

/// What the command line asked for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// `--view`: the camera at start (today's dev-only `#@lat,lon,zoom,pitch,bearing`, doc 06 §3.15).
    pub view: Option<ViewState>,
    /// `--basemap`.
    pub basemap: Option<Basemap>,
    /// `--size`: CSS pixels.
    pub size: Option<[u32; 2]>,
    /// `--scale`: a shot's scale factor.
    pub scale: Option<f64>,
    /// `--now`: Unix milliseconds.
    pub now_ms: Option<f64>,
    /// `--fixture`.
    pub fixture: Option<PathBuf>,
    /// `--demo`.
    pub demo: bool,
    /// `--screenshot`: the PNG to write.
    pub screenshot: Option<PathBuf>,
    /// `--wait`: how long a shot waits for loading layers (`headless::max_wait` without it).
    pub wait: Option<std::time::Duration>,
    /// `--ui`: the shot draws the shell, panels and controls as the window does
    /// ([`app::Engine::shell`]; plan section 8's `--ui`, with the state from `--fixture`, `--demo`
    /// and `--select` rather than named fixtures).
    pub ui: bool,
    /// `--select`: the flight a shot opens with selected, as a click on it selects it.
    pub select: Option<u32>,
    /// `--mode`: a shot's focus, as a saved `settings.json` would give it (the reference pictures'
    /// `mode`, docs/port/reference/README.md).
    pub mode: Option<rustradar_core::settings::Mode>,
    /// `--updated`: the updater started this run after installing it, so the launch does not look
    /// for an update again (`UPDATED_FLAG`, item P3; the Tauri updater passes it too).
    pub updated: bool,
}

/// Why the command line was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgsError {
    /// `-h` or `--help`: print [`USAGE`].
    Help,
    /// What was wrong.
    Bad(String),
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgsError::Help => f.write_str(USAGE),
            ArgsError::Bad(why) => write!(f, "{why}\n\n{USAGE}"),
        }
    }
}

impl std::error::Error for ArgsError {}

impl Options {
    /// Reads the arguments after the program's name. A flag given twice takes the last value.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, ArgsError> {
        let bad = |why: String| ArgsError::Bad(why);
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| bad(format!("{flag} needs a value")));
            match flag.as_str() {
                "--view" => options.view = Some(parse_view(&value()?).map_err(bad)?),
                "--basemap" => {
                    let v = value()?;
                    options.basemap = Some(match v.as_str() {
                        "dark" => Basemap::Dark,
                        "satellite" => Basemap::Satellite,
                        "realistic" => Basemap::Realistic,
                        _ => return Err(bad(format!("--basemap wants dark, satellite or realistic, not {v:?}"))),
                    });
                }
                "--size" => {
                    let v = value()?;
                    let size = v
                        .split_once('x')
                        .and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?]))
                        .filter(|&[w, h]: &[u32; 2]| w > 0 && h > 0)
                        .ok_or_else(|| bad(format!("--size wants WIDTHxHEIGHT in pixels, e.g. 1100x720, not {v:?}")))?;
                    options.size = Some(size);
                }
                "--scale" => {
                    let v = value()?;
                    let scale = v
                        .parse()
                        .ok()
                        .filter(|s: &f64| s.is_finite() && *s > 0.0)
                        .ok_or_else(|| bad(format!("--scale wants a positive number, e.g. 2, not {v:?}")))?;
                    options.scale = Some(scale);
                }
                "--now" => {
                    let v = value()?;
                    let now = v
                        .parse()
                        .ok()
                        .filter(|t: &f64| t.is_finite() && *t >= 0.0)
                        .ok_or_else(|| bad(format!("--now wants Unix milliseconds, e.g. 1760000000000, not {v:?}")))?;
                    options.now_ms = Some(now);
                }
                "--fixture" => {
                    let dir = PathBuf::from(value()?);
                    if !dir.is_dir() {
                        return Err(bad(format!("--fixture: there is no folder {}", dir.display())));
                    }
                    options.fixture = Some(dir);
                }
                "--demo" => options.demo = true,
                "--screenshot" => options.screenshot = Some(PathBuf::from(value()?)),
                "--wait" => {
                    let v = value()?;
                    let wait = v
                        .parse()
                        .ok()
                        .and_then(|s: f64| std::time::Duration::try_from_secs_f64(s).ok())
                        .ok_or_else(|| bad(format!("--wait wants seconds, e.g. 30, not {v:?}")))?;
                    options.wait = Some(wait);
                }
                "--ui" => options.ui = true,
                "--updated" => options.updated = true,
                "--mode" => {
                    use rustradar_core::settings::Mode;
                    let v = value()?;
                    options.mode = Some(match v.as_str() {
                        "aircraft" => Mode::Aircraft,
                        "trains" => Mode::Trains,
                        "ships" => Mode::Ships,
                        _ => return Err(bad(format!("--mode wants aircraft, trains or ships, not {v:?}"))),
                    });
                }
                "--select" => {
                    let v = value()?;
                    options.select = Some(v.parse().map_err(|_| bad(format!("--select wants a flight's id, e.g. 1001, not {v:?}")))?);
                }
                "-h" | "--help" => return Err(ArgsError::Help),
                _ => return Err(bad(format!("unknown argument {flag:?}"))),
            }
        }
        if options.scale.is_some() && options.screenshot.is_none() {
            return Err(bad("--scale is for --screenshot; the window takes the display's scale".into()));
        }
        if options.wait.is_some() && options.screenshot.is_none() {
            return Err(bad("--wait is for --screenshot".into()));
        }
        if (options.ui || options.select.is_some() || options.mode.is_some()) && options.screenshot.is_none() {
            return Err(bad("--ui, --select and --mode are for --screenshot; the window always draws the UI and keeps the saved mode".into()));
        }
        Ok(options)
    }

    /// What the state takes from the command line.
    pub fn startup(&self) -> Startup {
        Startup { basemap: self.basemap, fixture: self.fixture.clone(), demo: self.demo }
    }
}

/// `LAT,LON,ZOOM[,PITCH[,BEARING]]`, as today's dev address `#@lat,lon,zoom,pitch,bearing` reads
/// it (doc 06 §3.15: zoom and pitch unsigned; pitch and bearing 0 when left out).
fn parse_view(text: &str) -> Result<ViewState, String> {
    let wrong = || format!("--view wants LAT,LON,ZOOM[,PITCH[,BEARING]] in degrees, e.g. 59.33,18.07,9, not {text:?}");
    let numbers = text.split(',').map(|n| n.trim().parse::<f64>().ok().filter(|v| v.is_finite())).collect::<Option<Vec<_>>>().ok_or_else(wrong)?;
    let [latitude, longitude, zoom, rest @ ..] = numbers.as_slice() else { return Err(wrong()) };
    if rest.len() > 2 {
        return Err(wrong());
    }
    let pitch = rest.first().copied().unwrap_or(0.0);
    let bearing = rest.get(1).copied().unwrap_or(0.0);
    if !(-90.0..=90.0).contains(latitude) {
        return Err(format!("--view: latitude {latitude} is not within ±90"));
    }
    if !(-180.0..=180.0).contains(longitude) {
        return Err(format!("--view: longitude {longitude} is not within ±180"));
    }
    if *zoom < 0.0 || pitch < 0.0 {
        return Err(format!("--view: zoom and pitch cannot be negative in {text:?}"));
    }
    Ok(ViewState { longitude: *longitude, latitude: *latitude, zoom: *zoom, bearing, pitch, position: None })
}

/// Entry point the Android activity calls on its own thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;
    let mut builder = winit::event_loop::EventLoop::<app::UserEvent>::with_user_event();
    builder.with_android_app(android_app);
    if let Err(e) = app::run_with(builder, Options::default()) {
        log::error!("rustradar: {e}");
    }
}

/// Entry point iOS's `main.m` calls on the main thread (it does not return).
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn rustradar_ios_main() {
    platform::ios::init();
    if let Err(e) = app::run(Options::default()) {
        log::error!("rustradar: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, ArgsError> {
        Options::parse(args.iter().map(|a| a.to_string()))
    }

    fn bad(args: &[&str]) -> String {
        match parse(args) {
            Err(ArgsError::Bad(why)) => why,
            other => panic!("{args:?} gave {other:?}"),
        }
    }

    #[test]
    fn no_arguments_is_the_plain_window() {
        assert_eq!(parse(&[]), Ok(Options::default()));
    }

    #[test]
    fn a_full_shot_command() {
        let o = parse(&[
            "--screenshot", "out.png", "--size", "1440x900", "--scale", "2", "--view", "59.6519,17.9186,13.5,45,30",
            "--basemap", "satellite", "--now", "1760000000000", "--demo",
        ])
        .expect("parsed");
        assert_eq!(o.screenshot, Some(PathBuf::from("out.png")));
        assert_eq!(o.size, Some([1440, 900]));
        assert_eq!(o.scale, Some(2.0));
        assert_eq!(o.view, Some(ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None }));
        assert_eq!(o.basemap, Some(Basemap::Satellite));
        assert_eq!(o.now_ms, Some(1.76e12));
        assert!(o.demo);
        assert_eq!(o.startup(), Startup { basemap: Some(Basemap::Satellite), fixture: None, demo: true });
    }

    #[test]
    fn a_view_takes_three_to_five_numbers() {
        let v = parse(&["--view", "35,10,1.9"]).expect("parsed").view.expect("view");
        assert_eq!(v, ViewState::INITIAL);
        let v = parse(&["--view", "0,179.5,3,20"]).expect("parsed").view.expect("view");
        assert_eq!((v.pitch, v.bearing), (20.0, 0.0));
        let v = parse(&["--view", "-33.9,-70.6,5,10,-45"]).expect("parsed").view.expect("view");
        assert_eq!((v.latitude, v.longitude, v.bearing), (-33.9, -70.6, -45.0));
        for wrong in ["35,10", "35,10,2,0,0,0", "35,ten,2", "", "35,10,inf", "95,10,2", "35,190,2", "35,10,-1", "35,10,2,-5"] {
            assert!(bad(&["--view", wrong]).starts_with("--view"), "{wrong:?}");
        }
    }

    #[test]
    fn wrong_values_say_what_was_wanted() {
        assert!(bad(&["--basemap", "light"]).contains("dark, satellite or realistic"));
        assert!(bad(&["--size", "1100"]).contains("WIDTHxHEIGHT"));
        assert!(bad(&["--size", "0x720"]).contains("WIDTHxHEIGHT"));
        assert!(bad(&["--screenshot", "a.png", "--scale", "0"]).contains("positive"));
        assert!(bad(&["--now", "-5"]).contains("Unix milliseconds"));
        assert!(bad(&["--fixture", "/no/such/folder/here"]).contains("no folder"));
        assert!(bad(&["--view"]).contains("needs a value"));
        assert!(bad(&["--frobnicate"]).contains("unknown argument"));
        assert!(bad(&["--scale", "2"]).contains("--screenshot"));
        assert_eq!(parse(&["--help"]), Err(ArgsError::Help));
        assert_eq!(parse(&["-h"]), Err(ArgsError::Help));
    }

    /// `--wait` (P1): a shot's wait for loading layers, in seconds, only with `--screenshot`.
    #[test]
    fn a_shot_s_wait_is_in_seconds() {
        use std::time::Duration;
        assert_eq!(parse(&["--screenshot", "a.png", "--wait", "12.5"]).expect("parsed").wait, Some(Duration::from_millis(12_500)));
        assert_eq!(parse(&["--screenshot", "a.png", "--wait", "0"]).expect("parsed").wait, Some(Duration::ZERO));
        assert_eq!(parse(&["--screenshot", "a.png"]).expect("parsed").wait, None);
        for wrong in ["-1", "soon", "inf", "NaN"] {
            assert!(bad(&["--screenshot", "a.png", "--wait", wrong]).contains("--wait wants seconds"), "{wrong:?}");
        }
        assert!(bad(&["--wait", "10"]).contains("--screenshot"));
    }

    /// `--ui` and `--select`: a shot with the window's UI over the map, a flight selected.
    #[test]
    fn a_shot_may_draw_the_ui_with_a_flight_selected() {
        let o = parse(&["--screenshot", "a.png", "--ui", "--select", "1001"]).expect("parsed");
        assert!(o.ui);
        assert_eq!(o.select, Some(1001));
        assert!(!parse(&["--screenshot", "a.png"]).expect("parsed").ui);
        assert!(bad(&["--screenshot", "a.png", "--select", "KLM76F"]).contains("--select wants a flight's id"));
        assert!(bad(&["--ui"]).contains("--screenshot"));
        assert!(bad(&["--select", "7"]).contains("--screenshot"));
        let o = parse(&["--screenshot", "a.png", "--mode", "ships"]).expect("parsed");
        assert_eq!(o.mode, Some(rustradar_core::settings::Mode::Ships));
        assert!(bad(&["--screenshot", "a.png", "--mode", "boats"]).contains("aircraft, trains or ships"));
        assert!(bad(&["--mode", "trains"]).contains("--screenshot"));
    }

    #[test]
    fn a_fixture_folder_is_kept() {
        let dir = env!("CARGO_MANIFEST_DIR");
        let o = parse(&["--fixture", dir]).expect("parsed");
        assert_eq!(o.startup().fixture, Some(PathBuf::from(dir)));
    }
}
