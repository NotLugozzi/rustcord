use rustcord_client::screen_capture::ScreenCapture;
use rustcord_client::screen_encode::{EncoderSettings, Vp8Encoder};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("rustcord_client=info").init();
    let out = std::env::args().nth(1).unwrap_or_else(|| "screenshare.ivf".into());

    let mut capture = ScreenCapture::start().await?;
    let mut encoder: Option<Vp8Encoder> = None;
    let mut packets: Vec<(Vec<u8>, u32)> = Vec::new();
    let mut raw_bytes = 0usize;
    let started = std::time::Instant::now();

    while let Some(frame) = capture.next_frame().await {
        raw_bytes += frame.pixels.len();
        let needs_new = encoder
            .as_ref()
            .is_none_or(|e| !e.matches_source(frame.width, frame.height, frame.layout));
        if needs_new {
            encoder = Some(Vp8Encoder::new(
                frame.width,
                frame.height,
                frame.layout,
                EncoderSettings::default(),
            )?);
            let (w, h) = encoder.as_ref().unwrap().target_size();
            println!("encoding {}x{} -> {w}x{h}", frame.width, frame.height);
        }
        let encoder = encoder.as_mut().unwrap();
        for packet in encoder.encode(&frame)? {
            packets.push((packet.data, packet.timestamp));
        }
        if packets.len() >= 90 {
            break;
        }
    }
    if let Some(encoder) = encoder.as_mut() {
        for packet in encoder.finish()? {
            packets.push((packet.data, packet.timestamp));
        }
    }

    let (w, h) = encoder.as_ref().map(|e| e.target_size()).unwrap_or((0, 0));
    let encoded: usize = packets.iter().map(|(d, _)| d.len()).sum();
    let mut file = std::fs::File::create(&out)?;
    write_ivf_header(&mut file, w, h, packets.len() as u32)?;
    for (i, (data, _)) in packets.iter().enumerate() {
        file.write_all(&(data.len() as u32).to_le_bytes())?;
        file.write_all(&(i as u64).to_le_bytes())?;
        file.write_all(data)?;
    }

    let secs = started.elapsed().as_secs_f32();
    println!("{} packets, {encoded} bytes encoded from {raw_bytes} raw ({:.0}x smaller)", packets.len(), raw_bytes as f64 / encoded.max(1) as f64);
    println!("{:.1} fps encoded, wrote {out}", packets.len() as f32 / secs);
    Ok(())
}

fn write_ivf_header(file: &mut std::fs::File, width: u32, height: u32, frames: u32) -> std::io::Result<()> {
    file.write_all(b"DKIF")?;
    file.write_all(&0u16.to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"VP80")?;
    file.write_all(&(width as u16).to_le_bytes())?;
    file.write_all(&(height as u16).to_le_bytes())?;
    file.write_all(&30u32.to_le_bytes())?;
    file.write_all(&1u32.to_le_bytes())?;
    file.write_all(&frames.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())
}
