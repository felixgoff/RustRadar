//! Compiles the Flightradar24 protobuf definitions.
//!
//! `protox` is a pure-Rust protobuf compiler, so no `protoc` binary is needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");

    let fds = protox::compile(["fr24/proto/v1.proto"], ["proto"])?;
    prost_build::Config::new()
        .include_file("_includes.rs")
        .compile_fds(fds)?;
    Ok(())
}
