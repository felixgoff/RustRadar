//! Native RustRadar client: a winit window, wgpu rendering and an egui UI.
//!
//! Frame structure (see [`renderer::Renderer::render`]):
//! 1. map pass: clears to [`theme::CANVAS`] and draws the map, multisampled
//!    with a depth/stencil buffer, resolving into the target;
//! 2. UI pass: egui drawn straight onto the resolved target (`LoadOp::Load`).
//!
//! The same renderer serves the window ([`app::run`]) and the offscreen
//! screenshot path ([`headless::render_frame`]).

pub mod app;
pub mod gpu;
pub mod headless;
pub mod map;
pub mod renderer;
pub mod theme;
pub mod ui;

pub use gpu::Gpu;
pub use renderer::Renderer;

/// Entry point the Android `NativeActivity` glue calls on its own thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;
    let mut builder = winit::event_loop::EventLoop::builder();
    builder.with_android_app(android_app);
    if let Err(e) = app::run_with(builder) {
        log::error!("rustradar: {e}");
    }
}
