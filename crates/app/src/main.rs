//! `rustradar-app`: opens the window, or with `--screenshot FILE.png` draws one shot without a
//! window and writes it (see `--help`).

// release builds on Windows open no console window (doc 07 §11.7)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use rustradar_app::{ArgsError, Options, USAGE, app, headless};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,rustradar_app=info")).init();
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(ArgsError::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match options.screenshot.clone() {
        Some(path) => headless::save(&options, &path).map(|shot| {
            let backend = if shot.backend { "backend started" } else { "without the backend" };
            println!("wrote {} ({}x{}) with {}, {backend}", path.display(), shot.image.width, shot.image.height, shot.adapter);
        }),
        None => app::run(options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
