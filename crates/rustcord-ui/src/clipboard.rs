use std::cell::RefCell;

thread_local! {
    static SHARED: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

pub fn is_gnome_wayland(desktop: Option<&str>, wayland_display: Option<&str>) -> bool {
    let gnome = desktop.is_some_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case("gnome")));
    gnome && wayland_display.is_some_and(|d| !d.is_empty())
}

fn note_desktop() {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    if is_gnome_wayland(desktop.as_deref(), wayland.as_deref()) {
        tracing::info!("GNOME doesn't offer the Wayland clipboard protocol, so the clipboard goes through XWayland");
    }
}

fn with_clipboard<R>(use_it: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error>) -> Result<R, String> {
    SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        if shared.is_none() {
            note_desktop();
            *shared = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        use_it(shared.as_mut().expect("clipboard opened")).map_err(|e| e.to_string())
    })
}

pub fn read_text() -> Option<String> {
    with_clipboard(|clipboard| clipboard.get_text()).ok()
}

pub fn read_image_as_png() -> Option<(String, Vec<u8>)> {
    let image = with_clipboard(|clipboard| clipboard.get_image()).ok()?;

    let width = image.width as u32;
    let height = image.height as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let buffer = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())?;

    let mut png_bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
    use image::ImageEncoder;
    encoder
        .write_image(buffer.as_raw(), width, height, image::ExtendedColorType::Rgba8)
        .ok()?;

    Some(("pasted-image.png".to_string(), png_bytes))
}

pub fn write_text(text: &str) -> bool {
    match with_clipboard(|clipboard| clipboard.set_text(text.to_string())) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("failed to copy to clipboard: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_gnome_wayland;

    #[test]
    fn gnome_is_recognised_only_under_wayland() {
        assert!(is_gnome_wayland(Some("GNOME"), Some("wayland-0")));
        assert!(is_gnome_wayland(Some("ubuntu:GNOME"), Some("wayland-0")));
        assert!(!is_gnome_wayland(Some("GNOME"), None));
        assert!(!is_gnome_wayland(Some("KDE"), Some("wayland-0")));
        assert!(!is_gnome_wayland(None, Some("wayland-0")));
    }
}

