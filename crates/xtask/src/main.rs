//! Developer and release tooling: `cargo xtask <command>`. Never shipped, never cross-compiled.

mod bump;
mod fixtures;
mod fonts;
mod icons;
mod manifest;
mod package;
mod sign;
mod testflight;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>

commands:
  vendor-fonts   decode the .woff2 font subsets to assets/fonts
  vendor-icons   write the Lucide icons of the UI to assets/icons/lucide
  fixtures       download the map's tile and data fixtures to crates/map/tests/data

release (run by CI):
  bump [patch|minor|major|x.y.z|--check]   show, change or check the app's version
  version        print the app's version
  latest-json    write the updater manifest for a folder of signed artifacts
  package        build and pack the desktop packages (deb, rpm, appimage), --sign to sign them
  sign           sign updater artifacts (TAURI_SIGNING_PRIVATE_KEY, ..._PASSWORD)
  testflight-age days since the newest TestFlight build (APPLE_API_*)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("vendor-fonts") => fonts::run(rest),
        Some("vendor-icons") => icons::run(rest),
        Some("fixtures") => fixtures::run(rest),
        Some("bump") => bump::run_bump(rest),
        Some("version") => bump::run_version(rest),
        Some("latest-json") => manifest::run(rest),
        Some("package") => package::run(rest),
        Some("sign") => sign::run(rest),
        Some("testflight-age") => testflight::run(rest),
        Some("help" | "--help" | "-h") | None => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}
