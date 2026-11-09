//! The interactive window: winit event loop, surface, and input into egui.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopBuilder};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::renderer::Renderer;

type Error = Box<dyn std::error::Error>;

/// Opens the window and runs until it is closed.
pub fn run() -> Result<(), Error> {
    run_with(EventLoop::builder())
}

/// Like [`run`], for platforms that must configure the event loop first (Android).
pub fn run_with(mut builder: EventLoopBuilder<()>) -> Result<(), Error> {
    let event_loop = builder.build()?;
    // The placeholder globe spins, so redraw every frame (paced by vsync).
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App { started: Instant::now(), view: None, error: None };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    started: Instant,
    view: Option<View>,
    /// Why start-up failed, reported once the loop has exited.
    error: Option<Error>,
}

/// Everything tied to the window.
struct View {
    window: Arc<Window>,
    gpu: Gpu,
    /// `None` while suspended: Android takes the native window away then.
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    egui_input: egui_winit::State,
}

impl View {
    fn new(event_loop: &ActiveEventLoop) -> Result<Self, Error> {
        let attributes = Window::default_attributes()
            .with_title("RustRadar")
            .with_inner_size(LogicalSize::new(1100.0, 720.0))
            .with_min_inner_size(LogicalSize::new(360.0, 480.0));
        let window = Arc::new(event_loop.create_window(attributes)?);

        // The GL backend needs the display connection up front (Wayland above all).
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        )));
        let surface = instance.create_surface(window.clone())?;
        let gpu = Gpu::for_surface(instance, &surface)?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("the graphics adapter cannot present to this window")?;
        // A gamma-space format keeps CSS colours and egui's blending exact (see Renderer::new).
        let formats = surface.get_capabilities(&gpu.adapter).formats;
        config.format = formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&gpu.device, &config);

        let renderer = Renderer::new(&gpu, config.format);
        let egui_input = egui_winit::State::new(
            renderer.egui_ctx().clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let info = gpu.adapter.get_info();
        log::info!("{} ({:?}), {:?}, MSAA x{}", info.name, info.backend, config.format, renderer.msaa_samples());
        Ok(Self { window, gpu, surface: Some(surface), config, renderer, egui_input })
    }

    fn resize(&mut self, width: u32, height: u32) {
        // A zero-sized surface (minimised window) is invalid: keep the old one until it grows back.
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.gpu.device, &self.config);
        }
    }

    fn redraw(&mut self, time: f32) {
        let Some(surface) = &self.surface else { return };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.gpu.device, &self.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface texture validation error");
                return;
            }
        };
        let target = frame.texture.create_view(&Default::default());
        let input = self.egui_input.take_egui_input(&self.window);
        let size = [self.config.width, self.config.height];
        let output = self.renderer.render(&self.gpu, &target, size, input, time);
        self.egui_input.handle_platform_output(&self.window, output);
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        match &mut self.view {
            None => match View::new(event_loop) {
                Ok(view) => self.view = Some(view),
                Err(e) => {
                    self.error = Some(e);
                    event_loop.exit();
                }
            },
            // Back from the background: the window has a new native surface.
            Some(view) if view.surface.is_none() => match view.gpu.instance.create_surface(view.window.clone()) {
                Ok(surface) => {
                    surface.configure(&view.gpu.device, &view.config);
                    view.surface = Some(surface);
                    view.window.request_redraw();
                }
                Err(e) => log::error!("cannot recreate the surface: {e}"),
            },
            Some(_) => {}
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Android destroys the native window here; the surface must go before it does.
        // Desktop never suspends, and iOS keeps its view, so only Android needs this.
        if cfg!(target_os = "android")
            && let Some(view) = &mut self.view
        {
            view.surface = None;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(view) = &mut self.view else { return };
        // egui sees every event first; `consumed` will gate map gestures once the map takes input.
        let _response = view.egui_input.on_window_event(&view.window, &event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => view.resize(size.width, size.height),
            WindowEvent::RedrawRequested => view.redraw(self.started.elapsed().as_secs_f32()),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
    }
}
