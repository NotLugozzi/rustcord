use std::collections::HashMap;
use std::time::{Duration, Instant};

const MAX_CONCURRENT_ANIMATIONS: usize = 3;

struct DecodedFrame {
    texture_id: imgui::TextureId,
    delay: Duration,
}

enum Entry {
    Static { texture_id: imgui::TextureId, size: [f32; 2] },
    Animated {
        frames: Vec<DecodedFrame>,
        size: [f32; 2],
        current_frame: usize,
        frame_started_at: Instant,
        animating: bool,
    },
    Pending,
    Failed,
}

#[derive(Default)]
pub struct TextureCache {
    entries: HashMap<String, Entry>,
    animating_count: usize,
}

impl TextureCache {
    pub fn request(&mut self, url: &str, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        if self.entries.contains_key(url) {
            return;
        }
        self.entries.insert(url.to_string(), Entry::Pending);
        let _ = cmd_tx.send(crate::backend::UiCommand::DownloadImage { url: url.to_string() });
    }

    pub fn ingest(&mut self, url: String, bytes: &[u8], device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut imgui_wgpu::Renderer) {
        let format = image::guess_format(bytes).ok();
        let is_gif = matches!(format, Some(image::ImageFormat::Gif));

        if is_gif {
            match decode_gif_frames(bytes) {
                Ok(frames) if !frames.is_empty() => {
                    let size = [frames[0].1.width() as f32, frames[0].1.height() as f32];
                    let decoded: Vec<DecodedFrame> = frames
                        .into_iter()
                        .map(|(delay, rgba)| DecodedFrame {
                            texture_id: upload_texture(device, queue, renderer, &rgba),
                            delay,
                        })
                        .collect();
                    let animating = self.animating_count < MAX_CONCURRENT_ANIMATIONS;
                    if animating {
                        self.animating_count += 1;
                    }
                    self.entries.insert(
                        url,
                        Entry::Animated {
                            frames: decoded,
                            size,
                            current_frame: 0,
                            frame_started_at: Instant::now(),
                            animating,
                        },
                    );
                }
                _ => {
                    self.entries.insert(url, Entry::Failed);
                }
            }
            return;
        }

        match image::load_from_memory(bytes) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as f32, rgba.height() as f32];
                let texture_id = upload_texture(device, queue, renderer, &rgba);
                self.entries.insert(url, Entry::Static { texture_id, size });
            }
            Err(e) => {
                tracing::warn!("failed to decode image {url}: {e}");
                self.entries.insert(url, Entry::Failed);
            }
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        for entry in self.entries.values_mut() {
            if let Entry::Animated { frames, current_frame, frame_started_at, animating, .. } = entry {
                if !*animating || frames.is_empty() {
                    continue;
                }
                let delay = frames[*current_frame].delay;
                if now.duration_since(*frame_started_at) >= delay {
                    *current_frame = (*current_frame + 1) % frames.len();
                    *frame_started_at = now;
                }
            }
        }
    }

    pub fn get(&self, url: &str) -> Option<(imgui::TextureId, [f32; 2])> {
        match self.entries.get(url)? {
            Entry::Static { texture_id, size } => Some((*texture_id, *size)),
            Entry::Animated { frames, current_frame, size, .. } => {
                frames.get(*current_frame).map(|f| (f.texture_id, *size))
            }
            Entry::Pending | Entry::Failed => None,
        }
    }

    pub fn is_failed(&self, url: &str) -> bool {
        matches!(self.entries.get(url), Some(Entry::Failed))
    }
}

fn decode_gif_frames(bytes: &[u8]) -> image::ImageResult<Vec<(Duration, image::RgbaImage)>> {
    use image::{codecs::gif::GifDecoder, AnimationDecoder};
    let decoder = GifDecoder::new(std::io::Cursor::new(bytes))?;
    let frames = decoder.into_frames().collect_frames()?;
    Ok(frames
        .into_iter()
        .map(|f| {
            let delay = Duration::from(f.delay());
            let delay = if delay.is_zero() { Duration::from_millis(100) } else { delay };
            (delay, f.into_buffer())
        })
        .collect())
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut imgui_wgpu::Renderer,
    rgba: &image::RgbaImage,
) -> imgui::TextureId {
    let (width, height) = rgba.dimensions();
    let texture_config = imgui_wgpu::TextureConfig {
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        label: Some("chat-image"),
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..Default::default()
    };
    let texture = imgui_wgpu::Texture::new(device, renderer, texture_config);
    texture.write(queue, rgba.as_raw(), width, height);
    renderer.textures.insert(texture)
}
