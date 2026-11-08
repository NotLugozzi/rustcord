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
                glyph_ranges: imgui::FontGlyphRanges::from_slice(TEXT_GLYPH_RANGES),
                ..Default::default()
            })
        };
        let fallback = |size_pixels: f32| FontSource::TtfData {
            data: include_bytes!("../assets/fonts/DejaVuSans.ttf"),
            size_pixels,
            config: Some(FontConfig {
                oversample_h: 2,
                oversample_v: 2,
                pixel_snap_h: true,
                size_pixels,
                glyph_ranges: imgui::FontGlyphRanges::from_slice(TEXT_GLYPH_RANGES),
                ..Default::default()
            }),
        };
        let regular = context.fonts().add_font(&[
            FontSource::TtfData {
                data: include_bytes!("../assets/fonts/Inter-Regular.ttf"),
                size_pixels: font_size,
                config: make_config(font_size),
            },
            fallback(font_size),
        ]);
        let bold = context.fonts().add_font(&[
            FontSource::TtfData {
                data: include_bytes!("../assets/fonts/Inter-Bold.ttf"),
                size_pixels: font_size,
                config: make_config(font_size),
            },
            fallback(font_size),
        ]);
        let italic = context.fonts().add_font(&[
            FontSource::TtfData {
                data: include_bytes!("../assets/fonts/Inter-Italic.ttf"),
                size_pixels: font_size,
                config: make_config(font_size),
            },
            fallback(font_size),
        ]);
        let bold_italic = context.fonts().add_font(&[
            FontSource::TtfData {
                data: include_bytes!("../assets/fonts/Inter-BoldItalic.ttf"),
                size_pixels: font_size,
                config: make_config(font_size),
            },
            fallback(font_size),
        ]);
        let mono = context.fonts().add_font(&[
            FontSource::TtfData {
                data: include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
                size_pixels: mono_size,
                config: make_config(mono_size),
            },
            fallback(mono_size),
        ]);

        let fonts = crate::ui::Fonts { regular, bold, italic, bold_italic, mono };

        let renderer_config = RendererConfig {
            texture_format: surface_desc.format,
            ..Default::default()
        };
        let renderer = Renderer::new(&mut context, &device, &queue, renderer_config);

        let imgui = ImguiState {
            context,
            platform,
            renderer,
            last_frame: Instant::now(),
            last_cursor: None,
            fonts,
        };

        let textures = TextureCache::default();

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
    needs_redraw: bool,
    typing_active: bool,
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
        if self.needs_redraw || active || animating || typing || uploading || !self.event_rx.is_empty() {
            window.window.request_redraw();
            self.needs_redraw = false;
        }
        let interval = if active { 16 } else if animating { 33 } else { 100 };
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(interval)));
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
        needs_redraw: true,
        typing_active: false,
    };

    event_loop.run_app(&mut app).expect("event loop failed");
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
