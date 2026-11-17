use std::sync::mpsc::{channel, Receiver, Sender};

pub const ICON_SIZE: u32 = 64;
const ICON_FILL: [u8; 3] = [88, 101, 242];
const DOT_FILL: [u8; 3] = [242, 63, 67];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
}

fn inside_rounded_square(x: f32, y: f32, size: f32, radius: f32) -> f32 {
    let half = size / 2.0;
    let qx = (x - half).abs() - (half - radius);
    let qy = (y - half).abs() - (half - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
    (0.5 - outside).clamp(0.0, 1.0)
}

pub fn icon_rgba(size: u32, unread: bool) -> Vec<u8> {
    let s = size as f32;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let coverage = inside_rounded_square(x as f32 + 0.5, y as f32 + 0.5, s, s * 0.28);
            let at = ((y * size + x) * 4) as usize;
            pixels[at..at + 3].copy_from_slice(&ICON_FILL);
            pixels[at + 3] = (coverage * 255.0) as u8;
        }
    }
    let glyph_size = (s * 0.6).round() as u32;
    if let Some(glyph) = crate::icons::rasterize(crate::icons::Icon::MessageCircle, glyph_size) {
        let offset = (size - glyph_size) / 2;
        for (gx, gy, pixel) in glyph.enumerate_pixels() {
            let alpha = f32::from(pixel.0[3]) / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let at = (((gy + offset) * size + gx + offset) * 4) as usize;
            for channel in 0..3 {
                pixels[at + channel] = (f32::from(pixels[at + channel]) * (1.0 - alpha) + 255.0 * alpha) as u8;
            }
        }
    }
    if unread {
        let radius = s * 0.2;
        let (cx, cy) = (s - radius, radius);
        for y in 0..size {
            for x in 0..size {
                let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let at = ((y * size + x) * 4) as usize;
                let ring = (radius + s * 0.06 + 0.5 - distance).clamp(0.0, 1.0);
                if ring > 0.0 {
                    pixels[at + 3] = (f32::from(pixels[at + 3]) * (1.0 - ring)) as u8;
                }
                let dot = (radius + 0.5 - distance).clamp(0.0, 1.0);
                if dot > 0.0 {
                    pixels[at..at + 3].copy_from_slice(&DOT_FILL);
                    pixels[at + 3] = (dot * 255.0) as u8;
                }
            }
        }
    }
    pixels
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{icon_rgba, TrayEvent, ICON_SIZE};
    use std::sync::mpsc::Sender;

    pub struct RustcordTray {
        events: Sender<TrayEvent>,
        unread: bool,
    }

    fn argb(unread: bool) -> ksni::Icon {
        let mut data = icon_rgba(ICON_SIZE, unread);
        for pixel in data.chunks_exact_mut(4) {
            pixel.rotate_right(1);
        }
        ksni::Icon { width: ICON_SIZE as i32, height: ICON_SIZE as i32, data }
    }

    impl ksni::Tray for RustcordTray {
        fn id(&self) -> String {
            "rustcord".into()
        }

        fn title(&self) -> String {
            "rustcord".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![argb(self.unread)]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip { title: if self.unread { "rustcord — unread messages".into() } else { "rustcord".into() }, ..Default::default() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.events.send(TrayEvent::Show);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            vec![
                StandardItem { label: "Open rustcord".into(), activate: Box::new(|tray: &mut Self| { let _ = tray.events.send(TrayEvent::Show); }), ..Default::default() }.into(),
                ksni::MenuItem::Separator,
                StandardItem { label: "Quit".into(), activate: Box::new(|tray: &mut Self| { let _ = tray.events.send(TrayEvent::Quit); }), ..Default::default() }.into(),
            ]
        }
    }

    pub struct Platform {
        handle: ksni::Handle<RustcordTray>,
        runtime: tokio::runtime::Handle,
    }

    impl Platform {
        pub fn start(events: Sender<TrayEvent>, runtime: &tokio::runtime::Handle) -> Option<Self> {
            use ksni::TrayMethods;
            let tray = RustcordTray { events, unread: false };
            match runtime.block_on(tray.spawn()) {
                Ok(handle) => Some(Self { handle, runtime: runtime.clone() }),
                Err(e) => {
                    tracing::info!("no system tray to show an icon in, so closing the window quits: {e}");
                    None
                }
            }
        }

        pub fn set_unread(&mut self, unread: bool) {
            let handle = self.handle.clone();
            self.runtime.spawn(async move {
                handle.update(|tray| tray.unread = unread).await;
            });
        }

        pub fn available(&self) -> bool {
            !self.handle.is_closed()
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod platform {
    use super::{icon_rgba, TrayEvent, ICON_SIZE};
    use std::sync::mpsc::Sender;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Platform {
        icon: TrayIcon,
        events: Sender<TrayEvent>,
        open_id: tray_icon::menu::MenuId,
        quit_id: tray_icon::menu::MenuId,
    }

    fn icon(unread: bool) -> Option<tray_icon::Icon> {
        tray_icon::Icon::from_rgba(icon_rgba(ICON_SIZE, unread), ICON_SIZE, ICON_SIZE).ok()
    }

    impl Platform {
        pub fn start(events: Sender<TrayEvent>, _runtime: &tokio::runtime::Handle) -> Option<Self> {
            let open = MenuItem::new("Open rustcord", true, None);
            let quit = MenuItem::new("Quit", true, None);
            let menu = Menu::new();
            menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]).ok()?;
            let built = TrayIconBuilder::new().with_tooltip("rustcord").with_icon(icon(false)?).with_menu(Box::new(menu)).build();
            match built {
                Ok(tray) => Some(Self { icon: tray, events, open_id: open.id().clone(), quit_id: quit.id().clone() }),
                Err(e) => {
                    tracing::info!("couldn't add a tray icon, so closing the window quits: {e}");
                    None
                }
            }
        }

        pub fn pump(&self) {
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    let _ = self.events.send(TrayEvent::Show);
                }
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == self.open_id {
                    let _ = self.events.send(TrayEvent::Show);
                } else if event.id == self.quit_id {
                    let _ = self.events.send(TrayEvent::Quit);
                }
            }
        }

        pub fn set_unread(&mut self, unread: bool) {
            let _ = self.icon.set_icon(icon(unread));
        }

        pub fn available(&self) -> bool {
            true
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod platform {
    use super::TrayEvent;
    use std::sync::mpsc::Sender;

    pub struct Platform;

    impl Platform {
        pub fn start(_events: Sender<TrayEvent>, _runtime: &tokio::runtime::Handle) -> Option<Self> {
            None
        }

        pub fn set_unread(&mut self, _unread: bool) {}

        pub fn available(&self) -> bool {
            false
        }
    }
}

pub struct Tray {
    platform: platform::Platform,
    events: Receiver<TrayEvent>,
    unread: bool,
}

impl Tray {
    pub fn start(runtime: &tokio::runtime::Handle) -> Option<Self> {
        let (sender, events): (Sender<TrayEvent>, Receiver<TrayEvent>) = channel();
        let platform = platform::Platform::start(sender, runtime)?;
        Some(Self { platform, events, unread: false })
    }

    pub fn next_event(&mut self) -> Option<TrayEvent> {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.platform.pump();
        self.events.try_recv().ok()
    }

    pub fn set_unread(&mut self, unread: bool) {
        if unread != self.unread {
            self.unread = unread;
            self.platform.set_unread(unread);
        }
    }

    pub fn available(&self) -> bool {
        self.platform.available()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(data: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }

    #[test]
    fn the_icon_is_a_rounded_tile_with_a_glyph() {
        let data = icon_rgba(ICON_SIZE, false);
        assert_eq!(data.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert_eq!(pixel(&data, ICON_SIZE, 0, 0)[3], 0, "the corner is rounded off");
        assert_eq!(pixel(&data, ICON_SIZE, 6, ICON_SIZE / 2), [ICON_FILL[0], ICON_FILL[1], ICON_FILL[2], 255]);
        assert!(data.chunks_exact(4).any(|p| p[0] > 240 && p[1] > 240 && p[2] > 240 && p[3] == 255), "the white glyph shows");
    }

    #[test]
    fn unread_adds_a_red_dot_in_the_top_right() {
        let plain = icon_rgba(ICON_SIZE, false);
        let unread = icon_rgba(ICON_SIZE, true);
        let corner = (ICON_SIZE - 8, 8);
        assert_eq!(pixel(&unread, ICON_SIZE, corner.0, corner.1)[..3], DOT_FILL);
        assert_ne!(pixel(&plain, ICON_SIZE, corner.0, corner.1)[..3], DOT_FILL);
    }
}
