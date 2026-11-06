use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

struct AudioState {
    info: spa::param::audio::AudioInfoRaw,
    sender: mpsc::Sender<Vec<i16>>,
}

/// Captures the default PipeWire sink monitor and forwards downmixed 48 kHz
/// mono PCM chunks. Runs until the matching screen share is stopped.
pub fn capture_system_audio(sender: mpsc::Sender<Vec<i16>>, sharing: Arc<AtomicBool>) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;
    let state = Rc::new(RefCell::new(AudioState { info: Default::default(), sender }));

    let stream = pw::stream::StreamRc::new(
        core,
        "rustcord-share-audio",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::STREAM_CAPTURE_SINK => "true",
        },
    ).map_err(|e| e.to_string())?;

    let format_state = state.clone();
    let frame_state = state.clone();
    let _listener = stream.add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() { return; }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else { return };
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw { return; }
            let mut state = format_state.borrow_mut();
            if let Err(error) = state.info.parse(param) {
                tracing::warn!("screen audio: could not parse PipeWire format: {error}");
            } else {
                tracing::info!("screen audio: capturing default output at {} Hz, {} channels", state.info.rate(), state.info.channels());
            }
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let state = frame_state.borrow();
            let channels = state.info.channels() as usize;
            if channels == 0 { return; }
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            let bytes = data.chunk().size() as usize;
            if bytes < 4 * channels { return; }
            let Some(samples) = data.data() else { return };
            let sample_count = (bytes / 4).min(samples.len() / 4);
            let frames = sample_count / channels;
            if frames == 0 { return; }
            let mut mono = Vec::with_capacity(frames);
            for frame in 0..frames {
                let base = frame * channels;
                let left = f32::from_le_bytes(samples[base * 4..base * 4 + 4].try_into().unwrap());
                let right = if channels > 1 {
                    f32::from_le_bytes(samples[(base + 1) * 4..(base + 1) * 4 + 4].try_into().unwrap())
                } else { left };
                let sample = ((left + right) * 0.5 * i16::MAX as f32).round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                mono.push(sample);
            }
            let _ = state.sender.try_send(mono);
        })
        .register()
        .map_err(|e| e.to_string())?;

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(48_000);
    info.set_channels(2);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    ).map_err(|e| e.to_string())?.0.into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or_else(|| "could not build the audio format descriptor".to_string())?];
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ).map_err(|e| e.to_string())?;
    tracing::debug!("screen audio: connected to the default PipeWire output monitor");

    let loop_ref = mainloop.clone();
    let _timer = mainloop.loop_().add_timer(move |_| {
        if !sharing.load(Ordering::Relaxed) { loop_ref.quit(); }
    });
    _timer.update_timer(Some(std::time::Duration::from_millis(100)), Some(std::time::Duration::from_millis(100)))
        .into_result().map_err(|e| format!("could not arm system-audio stop timer: {e}"))?;
    mainloop.run();
    Ok(())
}
