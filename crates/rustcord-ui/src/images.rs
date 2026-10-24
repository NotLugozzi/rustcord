use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const TEXTURE_BUDGET_BYTES: usize = 192 * 1024 * 1024;
const IN_USE_WINDOW: Duration = Duration::from_secs(2);
const MAX_ANIMATED_DIM: u32 = 640;
const MAX_ANIMATED_BYTES: usize = 48 * 1024 * 1024;

pub enum Decoded {
    Still(image::RgbaImage),
    Animated(Vec<(Duration, image::RgbaImage)>),
    Failed,
}

pub fn decode(bytes: &[u8]) -> Decoded {
    if matches!(image::guess_format(bytes), Ok(image::ImageFormat::Gif)) {
        return match decode_gif_frames(bytes) {
            Ok(frames) if frames.len() > 1 => Decoded::Animated(shrink_frames(frames)),
            Ok(mut frames) if !frames.is_empty() => Decoded::Still(frames.remove(0).1),
            _ => Decoded::Failed,
        };
    }
    match image::load_from_memory(bytes) {
        Ok(img) => Decoded::Still(img.to_rgba8()),
        Err(e) => {
            tracing::warn!("failed to decode image: {e}");
            Decoded::Failed
        }
    }
}

fn shrink_frames(mut frames: Vec<(Duration, image::RgbaImage)>) -> Vec<(Duration, image::RgbaImage)> {
    let (w, h) = frames[0].1.dimensions();
    let longest = w.max(h);
    if longest > MAX_ANIMATED_DIM {
        let scale = MAX_ANIMATED_DIM as f32 / longest as f32;
        let (nw, nh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
        for (_, f) in &mut frames {
            *f = image::imageops::resize(f, nw, nh, image::imageops::FilterType::Triangle);
        }
    }
    let (w, h) = frames[0].1.dimensions();
    if frames.len() * w as usize * h as usize * 4 > MAX_ANIMATED_BYTES {
        frames.truncate(1);
    }
    frames
}

struct DecodedFrame {
    texture_id: imgui::TextureId,
    delay: Duration,
}

enum Kind {
    Static { texture_id: imgui::TextureId },
    Animated {
        frames: Vec<DecodedFrame>,
        current_frame: usize,
        frame_started_at: Instant,
        visible: Cell<bool>,
    },
}

enum Entry {
    Loaded { kind: Kind, size: [f32; 2], bytes: usize, last_used: Cell<Instant> },
    Pending,
    Failed,
}

#[derive(Default)]
pub struct TextureCache {
    entries: HashMap<String, Entry>,
    total_bytes: usize,
    pub animating: bool,
}

impl TextureCache {
    pub fn request(&mut self, url: &str, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        if self.entries.contains_key(url) {
            return;
        }
        self.entries.insert(url.to_string(), Entry::Pending);
        let _ = cmd_tx.send(crate::backend::UiCommand::DownloadImage { url: url.to_string() });
    }

    pub fn ingest(&mut self, url: String, decoded: Decoded, device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut imgui_wgpu::Renderer) {
        let now = Instant::now();
        let entry = match decoded {
            Decoded::Failed => Entry::Failed,
            Decoded::Still(rgba) => Entry::Loaded {
                size: [rgba.width() as f32, rgba.height() as f32],
                bytes: rgba.as_raw().len(),
                kind: Kind::Static { texture_id: upload_texture(device, queue, renderer, &rgba) },
                last_used: Cell::new(now),
            },
            Decoded::Animated(frames) => {
                let size = [frames[0].1.width() as f32, frames[0].1.height() as f32];
                let bytes = frames.iter().map(|(_, f)| f.as_raw().len()).sum();
                let frames = frames
                    .into_iter()
                    .map(|(delay, rgba)| DecodedFrame { texture_id: upload_texture(device, queue, renderer, &rgba), delay })
                    .collect();
                Entry::Loaded {
                    size,
                    bytes,
                    kind: Kind::Animated { frames, current_frame: 0, frame_started_at: now, visible: Cell::new(false) },
                    last_used: Cell::new(now),
                }
            }
        };
        if let Entry::Loaded { bytes, .. } = &entry {
            self.total_bytes += bytes;
        }
        self.entries.insert(url, entry);
    }

    pub fn evict(&mut self, renderer: &mut imgui_wgpu::Renderer) {
        if self.total_bytes <= TEXTURE_BUDGET_BYTES {
            return;
        }
        let now = Instant::now();
        let mut candidates: Vec<(Instant, String)> = self
            .entries
            .iter()
            .filter_map(|(url, e)| match e {
                Entry::Loaded { last_used, .. } if now.duration_since(last_used.get()) > IN_USE_WINDOW => {
                    Some((last_used.get(), url.clone()))
                }
                _ => None,
            })
            .collect();
        candidates.sort();
        for (_, url) in candidates {
            if self.total_bytes <= TEXTURE_BUDGET_BYTES {
                break;
            }
            if let Some(Entry::Loaded { kind, bytes, .. }) = self.entries.remove(&url) {
                self.total_bytes -= bytes;
                match kind {
                    Kind::Static { texture_id } => {
                        renderer.textures.remove(texture_id);
                    }
                    Kind::Animated { frames, .. } => {
                        for f in frames {
                            renderer.textures.remove(f.texture_id);
                        }
                    }
                }
            }
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        self.animating = false;
        for entry in self.entries.values_mut() {
            if let Entry::Loaded { kind: Kind::Animated { frames, current_frame, frame_started_at, visible }, .. } = entry {
                if !visible.replace(false) {
                    *frame_started_at = now;
                    continue;
                }
                self.animating = true;
                if now.duration_since(*frame_started_at) >= frames[*current_frame].delay {
                    *current_frame = (*current_frame + 1) % frames.len();
                    *frame_started_at = now;
                }
            }
        }
    }

    pub fn get(&self, url: &str) -> Option<(imgui::TextureId, [f32; 2])> {
        self.lookup(url, true)
    }

    pub fn get_still(&self, url: &str) -> Option<(imgui::TextureId, [f32; 2])> {
        self.lookup(url, false)
    }

    fn lookup(&self, url: &str, animate: bool) -> Option<(imgui::TextureId, [f32; 2])> {
        let Entry::Loaded { kind, size, last_used, .. } = self.entries.get(url)? else { return None };
        last_used.set(Instant::now());
        match kind {
            Kind::Static { texture_id } => Some((*texture_id, *size)),
            Kind::Animated { frames, current_frame, visible, .. } => {
                if animate {
                    visible.set(true);
                    frames.get(*current_frame)
                } else {
                    frames.first()
                }
                .map(|f| (f.texture_id, *size))
            }
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
