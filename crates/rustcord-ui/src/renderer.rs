use crate::app_state::AppState;
use crate::backend::{BackendEvent, UiCommand};
use crate::drain_backend_events;
use crate::images::TextureCache;
use crate::ui;
use imgui::{FontConfig, FontSource};
use imgui_wgpu::{Renderer, RendererConfig};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use pollster::block_on;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct ArboardClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl imgui::ClipboardBackend for ArboardClipboard {
    fn get(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok()
    }

    fn set(&mut self, value: &str) {
        if let Some(clipboard) = self.clipboard.as_mut() {
            let _ = clipboard.set_text(value);
        }
    }
}

struct ImguiState {
    context: imgui::Context,
    platform: WinitPlatform,
    renderer: Renderer,
    last_frame: Instant,
    last_cursor: Option<imgui::MouseCursor>,
    fonts: crate::ui::Fonts,
}

struct AppWindow {
    device: wgpu::Device,
    queue: wgpu::Queue,
    window: Arc<Window>,
    surface_desc: wgpu::SurfaceConfiguration,
    surface: wgpu::Surface<'static>,
    imgui: ImguiState,
    textures: TextureCache,
}

impl AppWindow {
    fn new(event_loop: &ActiveEventLoop) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let attributes = Window::default_attributes()
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_title("rustcord");
        let window = Arc::new(event_loop.create_window(attributes).unwrap());

        let size = window.inner_size();
        let hidpi_factor = window.scale_factor();
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .unwrap();

        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();

        let surface_desc = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![wgpu::TextureFormat::Bgra8Unorm],
        };
        surface.configure(&device, &surface_desc);

        let mut context = imgui::Context::create();
        context.set_clipboard_backend(ArboardClipboard { clipboard: arboard::Clipboard::new().ok() });
        let mut platform = WinitPlatform::new(&mut context);
        platform.attach_window(context.io_mut(), &window, HiDpiMode::Default);
        context.set_ini_filename(None);

        let font_size = (16.0 * hidpi_factor) as f32;
        let mono_size = (14.0 * hidpi_factor) as f32;
        context.io_mut().font_global_scale = (1.0 / hidpi_factor) as f32;

        let make_config = |size_pixels: f32| {
            Some(FontConfig {
                oversample_h: 2,
                oversample_v: 2,
                pixel_snap_h: true,
                size_pixels,
                ..Default::default()
            })
        };
        let regular = context.fonts().add_font(&[FontSource::TtfData {
            data: include_bytes!("../assets/fonts/Inter-Regular.ttf"),
            size_pixels: font_size,
            config: make_config(font_size),
        }]);
        let bold = context.fonts().add_font(&[FontSource::TtfData {
            data: include_bytes!("../assets/fonts/Inter-Bold.ttf"),
            size_pixels: font_size,
            config: make_config(font_size),
        }]);
        let italic = context.fonts().add_font(&[FontSource::TtfData {
            data: include_bytes!("../assets/fonts/Inter-Italic.ttf"),
            size_pixels: font_size,
            config: make_config(font_size),
        }]);
        let bold_italic = context.fonts().add_font(&[FontSource::TtfData {
            data: include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"),
            size_pixels: font_size,
            config: make_config(font_size),
        }]);
        let mono = context.fonts().add_font(&[FontSource::TtfData {
            data: include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
            size_pixels: mono_size,
            config: make_config(mono_size),
        }]);

        let fonts = crate::ui::Fonts { regular, bold, italic, bold_italic, mono };

        let renderer_config = RendererConfig {
            texture_format: surface_desc.format,
            ..Default::default()
        };
        let renderer = Renderer::new(&mut context, &device, &queue, renderer_config);

        let mut imgui = ImguiState {
            context,
            platform,
            renderer,
            last_frame: Instant::now(),
            last_cursor: None,
            fonts,
        };

        let mut textures = TextureCache::default();
        for (ch, bytes) in crate::emoji::BUNDLED {
            textures.ingest(crate::emoji::url(*ch), crate::images::decode(bytes), &device, &queue, &mut imgui.renderer);
        }

        Self {
            device,
            queue,
            window,
            surface_desc,
            surface,
            imgui,
            textures,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_desc.width = width;
        self.surface_desc.height = height;
        self.surface.configure(&self.device, &self.surface_desc);
    }

    fn redraw(&mut self, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, event_rx: &mut UnboundedReceiver<BackendEvent>) {
        let image_downloads = drain_backend_events(state, event_rx, cmd_tx);
        for (url, image) in image_downloads {
            self.textures.ingest(url, image, &self.device, &self.queue, &mut self.imgui.renderer);
        }
        self.textures.evict(&mut self.imgui.renderer);
        self.textures.tick();

        let imgui = &mut self.imgui;
        let now = Instant::now();
        imgui.context.io_mut().update_delta_time(now - imgui.last_frame);
        imgui.last_frame = now;

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_desc);
                return;
            }
            Err(_) => return,
        };

        imgui
            .platform
            .prepare_frame(imgui.context.io_mut(), &self.window)
            .expect("prepare_frame failed");

        state.theme.apply(imgui.context.style_mut());

        let fonts = imgui.fonts;
        let frame_ui = imgui.context.frame();
        ui::draw(frame_ui, state, cmd_tx, &mut self.textures, &fonts);
        self.textures.flush_wanted(cmd_tx);

        if imgui.last_cursor != frame_ui.mouse_cursor() {
            imgui.last_cursor = frame_ui.mouse_cursor();
            imgui.platform.prepare_render(frame_ui, &self.window);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: state.theme.background[0] as f64,
                        g: state.theme.background[1] as f64,
                        b: state.theme.background[2] as f64,
                        a: state.theme.background[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        imgui
            .renderer
            .render(imgui.context.render(), &self.queue, &self.device, &mut rpass)
            .expect("imgui render failed");

        drop(rpass);
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

struct App {
    state: AppState,
    cmd_tx: UnboundedSender<UiCommand>,
    event_rx: UnboundedReceiver<BackendEvent>,
    window: Option<AppWindow>,
    theme_applied: bool,
    last_input: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let mut window = AppWindow::new(event_loop);
            if !self.theme_applied {
                self.state.theme.apply(window.imgui.context.style_mut());
                self.theme_applied = true;
            }
            self.window = Some(window);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.as_mut() else { return };

        match &event {
            WindowEvent::Resized(size) => {
                window.resize(size.width, size.height);
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {}
            WindowEvent::Focused(focused) => self.state.window_focused = *focused,
            _ => self.last_input = Instant::now(),
        }
        match &event {
            WindowEvent::RedrawRequested => {
                window.redraw(&mut self.state, &self.cmd_tx, &mut self.event_rx);
            }
            _ => {}
        }

        if let WindowEvent::MouseWheel {
            delta: winit::event::MouseScrollDelta::PixelDelta(pos),
            phase: winit::event::TouchPhase::Moved,
            ..
        } = &event
        {
            let scale = window.window.scale_factor();
            let lines_per_pixel = 1.0 / (53.0 * scale);
            let h = (pos.x * lines_per_pixel) as f32;
            let v = (pos.y * lines_per_pixel) as f32;
            window.imgui.context.io_mut().add_mouse_wheel_event([h, v]);
            return;
        }

        window.imgui.platform.handle_event::<()>(
            window.imgui.context.io_mut(),
            &window.window,
            &Event::WindowEvent { window_id, event },
        );
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, device_id: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        let Some(window) = self.window.as_mut() else { return };
        window.imgui.platform.handle_event::<()>(
            window.imgui.context.io_mut(),
            &window.window,
            &Event::DeviceEvent { device_id, event },
        );
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        rustcord_client::captcha::pump_main_thread();
        let Some(window) = self.window.as_mut() else { return };
        let busy = self.last_input.elapsed() < Duration::from_millis(400) || window.textures.animating;
        if busy {
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(100)));
        }
        window.window.request_redraw();
        window.imgui.platform.handle_event::<()>(
            window.imgui.context.io_mut(),
            &window.window,
            &Event::AboutToWait,
        );
    }
}

pub fn run(state: AppState, cmd_tx: UnboundedSender<UiCommand>, event_rx: UnboundedReceiver<BackendEvent>) {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        state,
        cmd_tx,
        event_rx,
        window: None,
        theme_applied: false,
        last_input: Instant::now(),
    };

    event_loop.run_app(&mut app).expect("event loop failed");
}
