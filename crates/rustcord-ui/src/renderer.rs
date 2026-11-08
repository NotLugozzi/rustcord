use crate::app_state::AppState;
use crate::backend::{BackendEvent, UiCommand};
use crate::drain_backend_events;
use crate::images::TextureCache;
use crate::ui;
use imgui::{FontConfig, FontSource};

const TEXT_GLYPH_RANGES: &[u32] = &[
    0x0020, 0x00FF,
    0x0100, 0x024F,
    0x0250, 0x02FF,
    0x0300, 0x036F,
    0x0370, 0x03FF,
    0x0400, 0x04FF,
    0x0E00, 0x0E7F,
    0x1D00, 0x1D7F,
    0x1E00, 0x1EFF,
    0x2000, 0x206F,
    0x2070, 0x209F,
    0x20A0, 0x20BF,
    0x2100, 0x214F,
    0x2190, 0x21FF,
    0x2200, 0x22FF,
    0x2300, 0x23FF,
    0x2460, 0x24FF,
    0x2500, 0x257F,
    0x2580, 0x259F,
    0x25A0, 0x25FF,
    0x2600, 0x26FF,
    0x2700, 0x27BF,
    0x27C0, 0x27EF,
    0x2900, 0x297F,
    0x2B00, 0x2BFF,
    0x3000, 0x303F,
    0xFF00, 0xFFEF,
    0,
];
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

const ACTIVE_CONTEXT: &str = "imgui context is active";

struct ImguiState {
    context: Option<imgui::Context>,
    parked: Option<imgui::SuspendedContext>,
    platform: WinitPlatform,
    renderer: Renderer,
    last_frame: Instant,
    last_cursor: Option<imgui::MouseCursor>,
    fonts: crate::ui::Fonts,
}

impl ImguiState {
    fn park(&mut self) {
        if let Some(context) = self.context.take() {
            self.parked = Some(context.suspend());
        }
    }

    fn unpark(&mut self) {
        if let Some(parked) = self.parked.take() {
            match parked.activate() {
                Ok(context) => self.context = Some(context),
                Err(parked) => self.parked = Some(parked),
            }
        }
    }
}

fn add_fonts(context: &mut imgui::Context, hidpi_factor: f64, custom: &crate::fonts::CustomFonts) -> crate::ui::Fonts {
    let font_size = (custom.size * hidpi_factor as f32).round();
    let mono_size = (14.0 * hidpi_factor) as f32;
    let config = |size_pixels: f32| {
        Some(FontConfig {
            oversample_h: 2,
            oversample_v: 2,
            pixel_snap_h: true,
            size_pixels,
            glyph_ranges: imgui::FontGlyphRanges::from_slice(TEXT_GLYPH_RANGES),
            ..Default::default()
        })
    };
    let source = |data: &'static [u8], size_pixels: f32| FontSource::TtfData { data, size_pixels, config: config(size_pixels) };
    let mut add = |custom: Option<&Vec<u8>>, builtin: &'static [u8], size_pixels: f32| {
        let mut sources = Vec::with_capacity(3);
        if let Some(data) = custom {
            sources.push(FontSource::TtfData { data, size_pixels, config: config(size_pixels) });
        }
        sources.push(source(builtin, size_pixels));
        sources.push(source(include_bytes!("../assets/fonts/DejaVuSans.ttf"), size_pixels));
        context.fonts().add_font(&sources)
    };
    let regular = add(custom.regular.as_ref(), include_bytes!("../assets/fonts/Inter-Regular.ttf"), font_size);
    let bold = add(custom.bold.as_ref(), include_bytes!("../assets/fonts/Inter-Bold.ttf"), font_size);
    let italic = add(custom.italic.as_ref(), include_bytes!("../assets/fonts/Inter-Italic.ttf"), font_size);
    let bold_italic = add(custom.bold_italic.as_ref(), include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"), font_size);
    let mono = add(custom.mono.as_ref(), include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"), mono_size);
    crate::ui::Fonts { regular, bold, italic, bold_italic, mono }
}

fn build_imgui(mut context: imgui::Context, window: &Window, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> ImguiState {
    let hidpi_factor = window.scale_factor();
    context.set_clipboard_backend(ArboardClipboard { clipboard: arboard::Clipboard::new().ok() });
    let mut platform = WinitPlatform::new(&mut context);
    platform.attach_window(context.io_mut(), window, HiDpiMode::Default);
    context.set_ini_filename(None);

    context.io_mut().font_global_scale = (1.0 / hidpi_factor) as f32;
    let fonts = add_fonts(&mut context, hidpi_factor, &crate::fonts::CustomFonts { size: crate::fonts::DEFAULT_SIZE, ..Default::default() });
    let renderer = Renderer::new(&mut context, device, queue, RendererConfig { texture_format: format, ..Default::default() });
    ImguiState { context: Some(context), parked: None, platform, renderer, last_frame: Instant::now(), last_cursor: None, fonts }
}

const FRAME_HISTORY: usize = 300;

struct AppWindow {
    last_redraw: Instant,
    loaded_fonts: Option<crate::theme::ThemeFonts>,
    instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    frames: std::collections::VecDeque<(Instant, crate::devtools::FrameSample)>,
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



        let imgui = build_imgui(imgui::Context::create(), &window, &device, &queue, surface_desc.format);

        let textures = TextureCache::default();

        Self {
            last_redraw: Instant::now(),
            loaded_fonts: None,
            instance,
            adapter_info: adapter.get_info(),
            frames: std::collections::VecDeque::new(),
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
        let media = drain_backend_events(state, event_rx, cmd_tx);
        self.textures.sync_icons(self.window.scale_factor() as f32, &self.device, &self.queue, &mut self.imgui.renderer);
        for (url, image) in media.images {
            self.textures.ingest(url, image, &self.device, &self.queue, &mut self.imgui.renderer);
        }
        #[cfg(feature = "screenshare")]
        {
            for (user_id, image) in media.remote_videos {
                self.textures.ingest_live_video(user_id, &image, &self.device, &self.queue, &mut self.imgui.renderer);
            }
            self.textures.expire_live_videos(&mut self.imgui.renderer);
        }
        #[cfg(feature = "video")]
        {
            for batch in media.videos {
                self.textures.ingest_video_batch(batch, &mut self.imgui.renderer);
            }
            self.textures.upload_pending_video_frames(&self.device, &self.queue, &mut self.imgui.renderer);
        }
        #[cfg(feature = "video")]
        self.textures.evict(&mut self.imgui.renderer, cmd_tx);
        #[cfg(not(feature = "video"))]
        self.textures.evict(&mut self.imgui.renderer);
        #[cfg(feature = "video")]
        self.textures.tick(&mut self.imgui.renderer, cmd_tx);
        #[cfg(not(feature = "video"))]
        self.textures.tick();

        self.last_redraw = Instant::now();
        self.sync_theme_fonts(state, cmd_tx);
        let textures = &mut self.textures;
        let sample = render_frame(&mut self.imgui, &self.window, &self.surface, &self.surface_desc, &self.device, &self.queue, Some(&state.theme.clone()), |frame_ui, fonts| {
            ui::draw(frame_ui, state, cmd_tx, textures, &fonts);
            textures.flush_wanted(cmd_tx);
        });
        if let Some(sample) = sample {
            if self.frames.len() >= FRAME_HISTORY {
                self.frames.pop_front();
            }
            self.frames.push_back((Instant::now(), sample));
        }
    }

    fn sync_theme_fonts(&mut self, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
        if state.theme.fonts == self.loaded_fonts {
            return;
        }
        let failed = |spec: &crate::theme::FontSpec| matches!(state.font_fetches.get(spec), Some(Some(_)));
        match crate::fonts::resolve(state.theme.fonts.as_ref(), &failed) {
            crate::fonts::Resolution::Ready(custom) => {
                let context = self.imgui.context.as_mut().expect(ACTIVE_CONTEXT);
                context.fonts().clear();
                self.imgui.fonts = add_fonts(context, self.window.scale_factor(), &custom);
                self.imgui.renderer.reload_font_texture(context, &self.device, &self.queue);
                self.loaded_fonts = state.theme.fonts.clone();
                tracing::info!(custom = custom.regular.is_some(), size = custom.size, "theme fonts loaded");
            }
            crate::fonts::Resolution::Waiting(missing) => {
                for spec in missing {
                    if !state.font_fetches.contains_key(&spec) {
                        state.font_fetches.insert(spec.clone(), None);
                        let _ = cmd_tx.send(UiCommand::FetchFont { spec });
                    }
                }
            }
        }
    }

    fn runtime_snapshot(&self, state: &AppState) -> crate::devtools::RuntimeSnapshot {
        let info = &self.adapter_info;
        let mut calls = Vec::new();
        if state.connected_voice_channel.is_some() {
            calls.push(crate::devtools::CallSnapshot { label: "Voice call", current: state.voice_stats.clone(), history: state.voice_history.iter().cloned().collect() });
            if !state.screen_share_history.is_empty() {
                calls.push(crate::devtools::CallSnapshot {
                    label: "Screen share connection",
                    current: state.screen_share_stats.clone(),
                    history: state.screen_share_history.iter().cloned().collect(),
                });
            }
        }
        crate::devtools::RuntimeSnapshot {
            frames: self.frames.iter().copied().collect(),
            devtools_frames: Vec::new(),
            adapter: info.name.clone(),
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: format!("{} {}", info.driver, info.driver_info).trim().to_string(),
            surface: [self.surface_desc.width, self.surface_desc.height],
            format: format!("{:?}", self.surface_desc.format),
            present_mode: format!("{:?}", self.surface_desc.present_mode),
            textures: self.textures.stats(),
            cached_messages: state.messages.values().map(Vec::len).sum(),
            calls,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_frame(
    imgui: &mut ImguiState,
    window: &Window,
    surface: &wgpu::Surface<'static>,
    surface_desc: &wgpu::SurfaceConfiguration,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    theme: Option<&crate::theme::Theme>,
    draw: impl FnOnce(&imgui::Ui, crate::ui::Fonts),
) -> Option<crate::devtools::FrameSample> {
    let ms = |from: Instant, to: Instant| (to - from).as_secs_f32() * 1000.0;
    let now = Instant::now();
    imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut().update_delta_time(now - imgui.last_frame);
    imgui.last_frame = now;

    let frame = match surface.get_current_texture() {
        Ok(frame) => frame,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            surface.configure(device, surface_desc);
            return None;
        }
        Err(_) => return None,
    };
    let acquired = Instant::now();

    imgui
        .platform
        .prepare_frame(imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut(), window)
        .expect("prepare_frame failed");

    if let Some(theme) = theme {
        theme.apply(imgui.context.as_mut().expect(ACTIVE_CONTEXT).style_mut());
    }
    let clear = theme.map_or([0.06, 0.06, 0.07, 1.0], |t| t.background);

    let fonts = imgui.fonts;
    let frame_ui = imgui.context.as_mut().expect(ACTIVE_CONTEXT).frame();
    draw(frame_ui, fonts);
    let built = Instant::now();

    if imgui.last_cursor != frame_ui.mouse_cursor() {
        imgui.last_cursor = frame_ui.mouse_cursor();
        imgui.platform.prepare_render(frame_ui, window);
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    let draw_data = imgui.context.as_mut().expect(ACTIVE_CONTEXT).render();
    let mut sample = crate::devtools::FrameSample {
        vertices: draw_data.total_vtx_count.max(0) as u32,
        indices: draw_data.total_idx_count.max(0) as u32,
        draw_lists: draw_data.draw_lists_count() as u32,
        commands: draw_data.draw_lists().map(|list| list.commands().count() as u32).sum(),
        ..Default::default()
    };
    imgui.renderer.render(draw_data, queue, device, &mut rpass).expect("imgui render failed");

    drop(rpass);
    queue.submit(Some(encoder.finish()));
    let submitted = Instant::now();
    frame.present();
    let presented = Instant::now();
    sample.acquire_ms = ms(now, acquired);
    sample.build_ms = ms(acquired, built);
    sample.submit_ms = ms(built, submitted);
    sample.present_ms = ms(submitted, presented);
    Some(sample)
}

struct DevtoolsWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_desc: wgpu::SurfaceConfiguration,
    imgui: ImguiState,
    view: crate::devtools::DevtoolsView,
    last_input: Instant,
    last_draw: Instant,
    frames: std::collections::VecDeque<(Instant, crate::devtools::FrameSample)>,
}

impl DevtoolsWindow {
    fn open(event_loop: &ActiveEventLoop, main: &mut AppWindow) -> Option<Self> {
        let attributes = Window::default_attributes().with_inner_size(LogicalSize::new(1100.0, 720.0)).with_title("rustcord devtools");
        let window = Arc::new(event_loop.create_window(attributes).map_err(|e| tracing::warn!("devtools window failed: {e}")).ok()?);
        let surface = main.instance.create_surface(window.clone()).map_err(|e| tracing::warn!("devtools surface failed: {e}")).ok()?;
        let size = window.inner_size();
        let surface_desc = wgpu::SurfaceConfiguration { width: size.width.max(1), height: size.height.max(1), ..main.surface_desc.clone() };
        surface.configure(&main.device, &surface_desc);
        main.imgui.park();
        let imgui = match imgui::SuspendedContext::create().activate() {
            Ok(context) => {
                let mut imgui = build_imgui(context, &window, &main.device, &main.queue, surface_desc.format);
                imgui.park();
                Some(imgui)
            }
            Err(_) => None,
        };
        main.imgui.unpark();
        Some(Self {
            window,
            surface,
            surface_desc,
            imgui: imgui?,
            view: crate::devtools::DevtoolsView::default(),
            last_input: Instant::now(),
            last_draw: Instant::now(),
            frames: std::collections::VecDeque::new(),
        })
    }

    fn with_active<R>(&mut self, main: &mut AppWindow, f: impl FnOnce(&mut Self, &AppWindow) -> R) -> R {
        main.imgui.park();
        self.imgui.unpark();
        let result = f(self, main);
        self.imgui.park();
        main.imgui.unpark();
        result
    }

    fn redraw(&mut self, main: &mut AppWindow, state: &AppState) {
        self.last_draw = Instant::now();
        let mut runtime = main.runtime_snapshot(state);
        runtime.devtools_frames = self.frames.iter().copied().collect();
        let sample = self.with_active(main, |dev, main| {
            let view = &mut dev.view;
            render_frame(&mut dev.imgui, &dev.window, &dev.surface, &dev.surface_desc, &main.device, &main.queue, None, |frame_ui, fonts| {
                crate::devtools::draw(frame_ui, view, fonts.mono, &runtime);
            })
        });
        if let Some(sample) = sample {
            if self.frames.len() >= FRAME_HISTORY {
                self.frames.pop_front();
            }
            self.frames.push_back((Instant::now(), sample));
        }
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_desc.width = width;
        self.surface_desc.height = height;
        self.surface.configure(device, &self.surface_desc);
    }
}

struct App {
    state: AppState,
    cmd_tx: UnboundedSender<UiCommand>,
    event_rx: UnboundedReceiver<BackendEvent>,
    window: Option<AppWindow>,
    devtools: Option<DevtoolsWindow>,
    theme_applied: bool,
    last_input: Instant,
    needs_redraw: bool,
    typing_active: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let mut window = AppWindow::new(event_loop);
            if !self.theme_applied {
                self.state.theme.apply(window.imgui.context.as_mut().expect(ACTIVE_CONTEXT).style_mut());
                self.theme_applied = true;
            }
            self.window = Some(window);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.as_mut() else { return };
        if let Some(dev) = self.devtools.as_mut().filter(|dev| dev.window.id() == window_id) {
            match &event {
                WindowEvent::CloseRequested => {
                    self.devtools = None;
                    rustcord_client::devtools::set_capturing(false);
                    return;
                }
                WindowEvent::Resized(size) => dev.resize(&window.device, size.width, size.height),
                WindowEvent::RedrawRequested => {
                    dev.redraw(window, &self.state);
                    return;
                }
                _ => dev.last_input = Instant::now(),
            }
            dev.window.request_redraw();
            dev.with_active(window, |dev, _| {
                dev.imgui.platform.handle_event::<()>(
                    dev.imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut(),
                    &dev.window,
                    &Event::WindowEvent { window_id, event },
                );
            });
            return;
        }

        match &event {
            WindowEvent::Resized(size) => {
                window.resize(size.width, size.height);
                self.needs_redraw = true;
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.needs_redraw = false,
            WindowEvent::Focused(focused) => {
                self.state.window_focused = *focused;
                self.needs_redraw = true;
            }
            WindowEvent::DroppedFile(path) => {
                if let Some(channel_id) = self.state.selected_channel.clone() {
                    self.state.queue_attachment_path(&channel_id, path.clone());
                    self.state.focus_compose = true;
                }
                self.needs_redraw = true;
            }
            _ => {
                self.last_input = Instant::now();
                self.needs_redraw = true;
            }
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
            window.imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut().add_mouse_wheel_event([h, v]);
            return;
        }

        window.imgui.platform.handle_event::<()>(
            window.imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut(),
            &window.window,
            &Event::WindowEvent { window_id, event },
        );
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, device_id: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        let Some(window) = self.window.as_mut() else { return };
        window.imgui.platform.handle_event::<()>(
            window.imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut(),
            &window.window,
            &Event::DeviceEvent { device_id, event },
        );
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        rustcord_client::captcha::pump_main_thread();
        let Some(window) = self.window.as_mut() else { return };
        if std::mem::take(&mut self.state.devtools_requested) {
            match &self.devtools {
                Some(dev) => dev.window.focus_window(),
                None => {
                    self.devtools = DevtoolsWindow::open(event_loop, window);
                    if self.devtools.is_some() {
                        rustcord_client::devtools::set_capturing(true);
                    }
                }
            }
        }
        if let Some(dev) = &self.devtools {
            let interval = if dev.last_input.elapsed() < Duration::from_millis(400) { 33 } else { 250 };
            if dev.last_draw.elapsed() >= Duration::from_millis(interval) {
                dev.window.request_redraw();
            }
        }
        let active = self.last_input.elapsed() < Duration::from_millis(400);
        let animating = window.textures.animating || window.textures.has_wanted_icons();
        let typing = self.state.selected_channel.as_deref().is_some_and(|channel| {
            self.state.typing_users_for(channel).iter().any(|id| self.state.current_user.as_ref().is_none_or(|me| me.id != *id))
        });
        if self.typing_active && !typing {
            self.needs_redraw = true;
        }
        self.typing_active = typing;
        let uploading = !self.state.uploads.is_empty();
        let cadence = continuous_redraw_interval(active, animating || typing, uploading);
        let since_draw = window.last_redraw.elapsed();
        let due = cadence.is_some_and(|interval| since_draw >= interval);
        if self.needs_redraw || due || !self.event_rx.is_empty() {
            window.window.request_redraw();
            self.needs_redraw = false;
        }
        let wait = cadence.map_or(IDLE_WAKE, |interval| interval.saturating_sub(since_draw).max(Duration::from_millis(1)));
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
        window.imgui.platform.handle_event::<()>(
            window.imgui.context.as_mut().expect(ACTIVE_CONTEXT).io_mut(),
            &window.window,
            &Event::AboutToWait,
        );
    }
}

const IDLE_WAKE: Duration = Duration::from_millis(100);

fn continuous_redraw_interval(interacting: bool, animating: bool, uploading: bool) -> Option<Duration> {
    if interacting {
        Some(Duration::from_millis(16))
    } else if animating {
        Some(Duration::from_millis(33))
    } else if uploading {
        Some(Duration::from_millis(100))
    } else {
        None
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
        devtools: None,
        theme_applied: false,
        last_input: Instant::now(),
        needs_redraw: true,
        typing_active: false,
    };

    event_loop.run_app(&mut app).expect("event loop failed");
}

#[cfg(test)]
mod redraw_tests {
    use super::*;

    #[test]
    fn continuous_redraws_are_capped_by_what_is_happening() {
        assert_eq!(continuous_redraw_interval(true, true, true), Some(Duration::from_millis(16)));
        assert_eq!(continuous_redraw_interval(false, true, true), Some(Duration::from_millis(33)));
        assert_eq!(continuous_redraw_interval(false, false, true), Some(Duration::from_millis(100)));
        assert_eq!(continuous_redraw_interval(false, false, false), None);
    }
}

#[cfg(test)]
mod glyph_range_tests {
    use super::TEXT_GLYPH_RANGES;

    #[test]
    fn the_range_table_is_shaped_the_way_imgui_demands() {
        assert_eq!(TEXT_GLYPH_RANGES.len() % 2, 1, "must be odd (pairs plus terminator)");
        assert_eq!(TEXT_GLYPH_RANGES.last(), Some(&0), "must be zero terminated");
        assert!(TEXT_GLYPH_RANGES[..TEXT_GLYPH_RANGES.len() - 1].iter().all(|&c| c != 0));
        for pair in TEXT_GLYPH_RANGES[..TEXT_GLYPH_RANGES.len() - 1].chunks(2) {
            assert!(pair[0] <= pair[1], "range {:04X}..{:04X} is inverted", pair[0], pair[1]);
        }
    }

    #[test]
    fn the_ranges_never_overlap_or_sit_out_of_order() {
        let pairs: Vec<&[u32]> = TEXT_GLYPH_RANGES[..TEXT_GLYPH_RANGES.len() - 1].chunks(2).collect();
        for window in pairs.windows(2) {
            assert!(
                window[0][1] < window[1][0],
                "{:04X}..{:04X} overlaps or precedes {:04X}..{:04X}",
                window[0][0], window[0][1], window[1][0], window[1][1],
            );
        }
    }

    #[test]
    fn decorative_channel_name_characters_are_covered() {
        let covered = |ch: char| {
            TEXT_GLYPH_RANGES[..TEXT_GLYPH_RANGES.len() - 1]
                .chunks(2)
                .any(|p| (p[0]..=p[1]).contains(&(ch as u32)))
        };
        let mut missing: Vec<char> = Vec::new();
        for ch in "\u{2605}\u{FF61}\u{00B0}\u{02D6}\u{2727}\u{2726}\u{2500}\u{2501}\u{2661}\u{22C6}\u{2027}\u{208A}\u{02DA}\u{029A}\u{025E}\u{27E1}\u{0E51}\u{2022}\u{1D17}\u{2192}\u{2713}".chars() {
            if !covered(ch) {
                missing.push(ch);
            }
        }
        assert!(missing.is_empty(), "channel-name glyphs outside the ranges: {missing:?}");
    }
}
