//! Developer and release tooling: `cargo xtask <command>`. Never shipped, never cross-compiled.

mod fixtures;
mod fonts;
mod icons;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>

commands:
  vendor-fonts   decode the .woff2 font subsets to assets/fonts
  vendor-icons   write the Lucide icons of the UI to assets/icons/lucide
  fixtures       download the map's tile fixtures to crates/map/tests/data/tiles";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("vendor-fonts") => fonts::run(rest),
        Some("vendor-icons") => icons::run(rest),
        Some("fixtures") => fixtures::run(rest),
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
