//! `rustradar-app`: opens the window, or with `--screenshot out.png [--size WxH] [--scale F]`
//! renders one frame offscreen and writes it as a PNG.

use std::path::PathBuf;
use std::process::ExitCode;

use rustradar_app::{app, headless};

const USAGE: &str = "usage: rustradar-app [--screenshot FILE.png [--size WIDTHxHEIGHT] [--scale FACTOR]]";

struct Args {
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    scale: f32,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { screenshot: None, size: (1100, 720), scale: 1.0 };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value\n{USAGE}"));
        match flag.as_str() {
            "--screenshot" => args.screenshot = Some(value()?.into()),
            "--size" => {
                let v = value()?;
                args.size = v
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .ok_or(format!("--size wants WIDTHxHEIGHT in pixels, e.g. 1100x720, not {v:?}"))?;
            }
            "--scale" => {
                let v = value()?;
                args.scale = v
                    .parse()
                    .ok()
                    .filter(|s: &f32| s.is_finite() && *s > 0.0)
                    .ok_or(format!("--scale wants a positive number, e.g. 2, not {v:?}"))?;
            }
            "-h" | "--help" => return Err(USAGE.into()),
            _ => return Err(format!("unknown argument {flag:?}\n{USAGE}")),
        }
    }
    Ok(args)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    match args.screenshot {
        Some(path) => {
            let frame = headless::render_frame(args.size.0, args.size.1, args.scale)?;
            headless::save_png(&frame, &path)?;
            println!("wrote {} ({}x{}) with {}", path.display(), frame.width, frame.height, frame.adapter);
            Ok(())
        }
        None => app::run(),
    }
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,rustradar_app=info")).init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
