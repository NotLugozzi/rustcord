use rustcord_client::screen_capture::ScreenCapture;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("rustcord_client=info").init();
    let out = std::env::args().nth(1).unwrap_or_else(|| "capture-probe.ppm".into());

    let mut capture = ScreenCapture::start().await?;
    let mut seen = 0u32;
    let started = std::time::Instant::now();

    while let Some(frame) = capture.next_frame().await {
        seen += 1;
        if seen == 1 {
            let mut ppm = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
            let (r, b) = (frame.layout.red_offset(), frame.layout.blue_offset());
            for px in frame.pixels.chunks_exact(4) {
                ppm.extend_from_slice(&[px[r], px[1], px[b]]);
            }
            std::fs::write(&out, ppm)?;
            println!("wrote {out} ({}x{}, {:?})", frame.width, frame.height, frame.layout);
        }
        if seen >= 90 {
            break;
        }
    }

    let elapsed = started.elapsed().as_secs_f32();
    println!("{seen} frames in {elapsed:.1}s ({:.1} fps delivered)", seen as f32 / elapsed);
    Ok(())
}
