pub fn read_image_as_png() -> Option<(String, Vec<u8>)> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;

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
