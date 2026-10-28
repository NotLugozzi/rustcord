use crate::error::ClientError;
use crate::voice_sdp::{LocalIceParams, RemoteSdp};
use std::net::SocketAddr;
use std::time::Instant;
use str0m::config::Fingerprint;
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::rtp::Ssrc;
use str0m::{Candidate, Event, IceConnectionState, IceCreds, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;

pub const OPUS_PT: u8 = 120;
const OPUS_FRAME_SAMPLES: u32 = 960;

#[derive(Default, Clone)]
pub struct RtcStats {
    pub rtt_ms: Option<f32>,
    pub loss_up: Option<f32>,
    pub loss_down: Option<f32>,
    pub jitter_ms: Option<f32>,
}

pub enum RtcEvent {
    Connected,
    Disconnected,
    AudioData { ssrc: u32, payload: Vec<u8> },
}

pub struct VoiceRtc {
    rtc: Rtc,
    socket: std::sync::Arc<UdpSocket>,
    audio_mid: Mid,
    audio_ssrc: Ssrc,
    remote_audio_mids: Vec<(Mid, u32)>,
    rtp_timestamp: u32,
    rtp_packets_sent: u64,
    rtp_packets_received: u64,
    media_data_events: u64,
    pub stats: RtcStats,
    jitter_by_mid: std::collections::HashMap<Mid, u32>,
}

impl VoiceRtc {
    pub async fn new() -> Result<(Self, LocalIceParams), ClientError> {
        let mut config = RtcConfig::new().clear_codecs();
        config
            .codec_config()
            .add_config(Pt::from(OPUS_PT), None, Codec::Opus, Frequency::FORTY_EIGHT_KHZ, Some(2), Default::default());
        let mut rtc = config.set_stats_interval(Some(std::time::Duration::from_secs(1))).build(Instant::now());

        let local_ice = {
            let mut api = rtc.direct_api();
            api.set_ice_controlling(true);
            let local_fingerprint = api.local_dtls_fingerprint().clone();
            let local_creds = api.local_ice_credentials();
            LocalIceParams {
                ufrag: local_creds.ufrag.clone(),
                pwd: local_creds.pass.clone(),
                fingerprint: format_fingerprint(&local_fingerprint),
            }
        };

        let audio_mid = Mid::new();
        let audio_ssrc = {
            let mut api = rtc.direct_api();
            api.declare_media(audio_mid, MediaKind::Audio);
            let ssrc = api.new_ssrc();
            api.declare_stream_tx(ssrc, None, audio_mid, None);
            ssrc
        };

        let socket = std::sync::Arc::new(
            UdpSocket::bind("0.0.0.0:0")
                .await
                .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: udp bind failed: {e}")))?,
        );

        Ok((Self { rtc, socket, audio_mid, audio_ssrc, remote_audio_mids: Vec::new(), rtp_timestamp: 0, rtp_packets_sent: 0, rtp_packets_received: 0, media_data_events: 0, stats: RtcStats::default(), jitter_by_mid: Default::default() }, local_ice))
    }

    pub fn audio_ssrc(&self) -> u32 {
        *self.audio_ssrc
    }

    pub fn expect_remote_audio(&mut self, ssrc: u32) {
        if self.remote_audio_mids.iter().any(|(_, existing)| *existing == ssrc) { return; }
        let mid = Mid::new();
        let mut api = self.rtc.direct_api();
        api.declare_media(mid, MediaKind::Audio);
        api.expect_stream_rx(Ssrc::from(ssrc), None, mid, None);
        self.remote_audio_mids.push((mid, ssrc));
    }

    pub fn send_audio_frame(&mut self, opus_payload: &[u8]) -> Result<(), ClientError> {
        let pt = Pt::from(OPUS_PT);
        let media_time = MediaTime::new(self.rtp_timestamp as u64, Frequency::FORTY_EIGHT_KHZ);
        let writer = self
            .rtc
            .writer(self.audio_mid)
            .ok_or_else(|| ClientError::GatewayClosed("voice rtc: no writer for audio mid".into()))?;
        writer
            .write(pt, Instant::now(), media_time, opus_payload.to_vec())
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: audio write failed: {e}")))?;
        self.rtp_timestamp = self.rtp_timestamp.wrapping_add(OPUS_FRAME_SAMPLES);
        Ok(())
    }

    pub fn set_remote(&mut self, remote: &RemoteSdp) -> Result<(), ClientError> {
        {
            let mut api = self.rtc.direct_api();
            api.set_remote_ice_credentials(IceCreds { ufrag: remote.ice_ufrag.clone(), pass: remote.ice_pwd.clone() });
            let fingerprint_bytes: Vec<u8> = remote
                .fingerprint
                .split(':')
                .map(|h| u8::from_str_radix(h, 16).unwrap_or(0))
                .collect();
            api.set_remote_fingerprint(Fingerprint { hash_func: "sha-256".to_string(), bytes: fingerprint_bytes });
            api.start_dtls(true).map_err(|e| ClientError::GatewayClosed(format!("voice rtc: start_dtls failed: {e}")))?;
        }

        let candidate = remote
            .candidates
            .first()
            .ok_or_else(|| ClientError::GatewayClosed("voice rtc: no remote candidate".into()))?;
        let addr: SocketAddr = format!("{}:{}", candidate.ip, candidate.port)
            .parse()
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: bad candidate address: {e}")))?;
        let ice_candidate = Candidate::host(addr, candidate.transport.as_str())
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: bad candidate: {e}")))?;
        self.rtc.add_remote_candidate(ice_candidate);

        let local_ip = local_outbound_ip(addr)?;
        let local_port = self.socket.local_addr().map_err(|e| ClientError::GatewayClosed(format!("voice rtc: local_addr failed: {e}")))?.port();
        let base_addr = SocketAddr::new(local_ip, local_port);
        let local_candidate =
            Candidate::host(base_addr, "udp").map_err(|e| ClientError::GatewayClosed(format!("voice rtc: bad local candidate: {e}")))?;
        self.rtc.add_local_candidate(local_candidate);

        Ok(())
    }

    fn poll_output(&mut self) -> Result<PollOutcome, ClientError> {
        loop {
            let output = self
                .rtc
                .poll_output()
                .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: poll_output failed: {e}")))?;

            match output {
                Output::Timeout(deadline) => {
                    let now = Instant::now();
                    if deadline <= now {
                        self.rtc
                            .handle_input(Input::Timeout(now))
                            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: handle_input timeout failed: {e}")))?;
                        continue;
                    }
                    return Ok(PollOutcome::WaitUntil(deadline));
                }
                Output::Transmit(transmit) => {
                    let first_byte = transmit.contents.first().copied().unwrap_or(0);
                    if first_byte >= 128 {
                        self.rtp_packets_sent += 1;
                        if self.rtp_packets_sent % 50 == 1 {
                            tracing::trace!("voice rtc: transmitted {} rtp/rtcp-like packets so far (latest {} bytes, first byte {:#04x})", self.rtp_packets_sent, transmit.contents.len(), first_byte);
                        }
                    }
                    return Ok(PollOutcome::Transmit(transmit.destination, transmit.contents.to_vec()));
                }
                Output::Event(event) => match event {
                    Event::Connected => {
                        tracing::debug!("voice rtc: connected (ice+dtls established)");
                        return Ok(PollOutcome::Event(RtcEvent::Connected));
                    }
                    Event::IceConnectionStateChange(state) => {
                        if state == IceConnectionState::Disconnected {
                            tracing::debug!("voice rtc: ice disconnected");
                            return Ok(PollOutcome::Event(RtcEvent::Disconnected));
                        }
                    }
                    Event::MediaData(data) => {
                        self.media_data_events += 1;
                        if self.media_data_events == 1 || self.media_data_events % 50 == 0 {
                            tracing::trace!("voice rtc: received {} MediaData events so far, latest mid {:?}, pt {:?}, {} bytes", self.media_data_events, data.mid, data.pt, data.data.len());
                        }
                        if let Some((_, ssrc)) = self.remote_audio_mids.iter().find(|(mid, _)| *mid == data.mid) {
                            return Ok(PollOutcome::Event(RtcEvent::AudioData { ssrc: *ssrc, payload: data.data.to_vec() }));
                        }
                        tracing::warn!(
                            "voice rtc: MediaData for unregistered mid {:?}, {} bytes; known mids {:?}",
                            data.mid,
                            data.data.len(),
                            self.remote_audio_mids,
                        );
                    }
                    Event::PeerStats(s) => {
                        self.stats.rtt_ms = s.rtt.map(|d| d.as_secs_f32() * 1000.0);
                        self.stats.loss_up = s.egress_loss_fraction.map(|f| f * 100.0);
                        self.stats.loss_down = s.ingress_loss_fraction.map(|f| f * 100.0);
                    }
                    Event::MediaIngressStats(s) => {
                        self.jitter_by_mid.insert(s.mid, s.jitter);
                        let worst = self.jitter_by_mid.values().copied().max().unwrap_or(0);
                        self.stats.jitter_ms = Some(worst as f32 / 48.0);
                    }
                    _ => {}
                },
            }
        }
    }

    pub async fn drive(&mut self) -> Result<DriveResult, ClientError> {
        loop {
            match self.poll_output()? {
                PollOutcome::WaitUntil(deadline) => return Ok(DriveResult::WaitUntil(deadline)),
                PollOutcome::Transmit(destination, contents) => {
                    if let Err(e) = self.socket.send_to(&contents, destination).await {
                        tracing::warn!("voice rtc: udp send failed: {e}");
                    }
                }
                PollOutcome::Event(event) => return Ok(DriveResult::Event(event)),
            }
        }
    }

    pub fn handle_timeout(&mut self) -> Result<(), ClientError> {
        self.rtc
            .handle_input(Input::Timeout(Instant::now()))
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: handle_input timeout failed: {e}")))
    }

    pub fn socket_handle(&self) -> std::sync::Arc<UdpSocket> {
        self.socket.clone()
    }

    pub fn handle_datagram(&mut self, source: SocketAddr, data: &[u8]) -> Result<(), ClientError> {
        let destination = self
            .socket
            .local_addr()
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: local_addr failed: {e}")))?;
        let Ok(receive) = Receive::new(Protocol::Udp, source, destination, data) else {
            tracing::warn!("voice rtc: failed to parse received datagram, {} bytes, first byte {:#04x}", data.len(), data.first().copied().unwrap_or(0));
            return Ok(());
        };
        let first_byte = data.first().copied().unwrap_or(0);
        if first_byte >= 128 {
            self.rtp_packets_received += 1;
            if self.rtp_packets_received % 50 == 1 {
                tracing::trace!("voice rtc: received {} rtp/rtcp-like packets so far (latest {} bytes, first byte {:#04x})", self.rtp_packets_received, data.len(), first_byte);
            }
        }
        self.rtc
            .handle_input(Input::Receive(Instant::now(), receive))
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: handle_input receive failed: {e}")))
    }
}

enum PollOutcome {
    WaitUntil(Instant),
    Transmit(SocketAddr, Vec<u8>),
    Event(RtcEvent),
}

pub enum DriveResult {
    WaitUntil(Instant),
    Event(RtcEvent),
}

fn local_outbound_ip(remote: SocketAddr) -> Result<std::net::IpAddr, ClientError> {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| ClientError::GatewayClosed(format!("voice rtc: probe bind failed: {e}")))?;
    probe.connect(remote).map_err(|e| ClientError::GatewayClosed(format!("voice rtc: probe connect failed: {e}")))?;
    Ok(probe.local_addr().map_err(|e| ClientError::GatewayClosed(format!("voice rtc: probe local_addr failed: {e}")))?.ip())
}

fn format_fingerprint(fp: &Fingerprint) -> String {
    fp.bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}
