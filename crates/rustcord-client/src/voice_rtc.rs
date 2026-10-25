use crate::error::ClientError;
use crate::voice_sdp::{LocalIceParams, RemoteSdp};
use std::net::SocketAddr;
use std::time::Instant;
use str0m::config::Fingerprint;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, IceCreds, Input, Output, Rtc};
use tokio::net::UdpSocket;

pub enum RtcEvent {
    Connected,
    Disconnected,
}

pub struct VoiceRtc {
    rtc: Rtc,
    socket: UdpSocket,
}

impl VoiceRtc {
    pub async fn new() -> Result<(Self, LocalIceParams), ClientError> {
        let mut rtc = Rtc::builder().build(Instant::now());

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

        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: udp bind failed: {e}")))?;

        Ok((Self { rtc, socket }, local_ice))
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

    pub async fn drive(&mut self, buf: &mut [u8]) -> Result<Option<RtcEvent>, ClientError> {
        loop {
            let output = match self.rtc.poll_output() {
                Ok(o) => o,
                Err(e) => return Err(ClientError::GatewayClosed(format!("voice rtc: poll_output failed: {e}"))),
            };

            match output {
                Output::Timeout(deadline) => {
                    let now = Instant::now();
                    if deadline <= now {
                        self.rtc
                            .handle_input(Input::Timeout(now))
                            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: handle_input timeout failed: {e}")))?;
                        continue;
                    }

                    let sleep_duration = deadline - now;
                    tokio::select! {
                        _ = tokio::time::sleep(sleep_duration) => {
                            self.rtc
                                .handle_input(Input::Timeout(Instant::now()))
                                .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: handle_input timeout failed: {e}")))?;
                        }
                        recv = self.socket.recv_from(buf) => {
                            let (len, source) = recv.map_err(|e| ClientError::GatewayClosed(format!("voice rtc: udp recv failed: {e}")))?;
                            let destination = self.socket.local_addr().map_err(|e| ClientError::GatewayClosed(format!("voice rtc: local_addr failed: {e}")))?;
                            if let Ok(receive) = Receive::new(Protocol::Udp, source, destination, &buf[..len]) {
                                self.rtc
                                    .handle_input(Input::Receive(Instant::now(), receive))
                                    .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: handle_input receive failed: {e}")))?;
                            } else {
                                tracing::warn!("voice rtc: failed to parse received datagram");
                            }
                        }
                    }
                }
                Output::Transmit(transmit) => {
                    if let Err(e) = self.socket.send_to(&transmit.contents, transmit.destination).await {
                        tracing::warn!("voice rtc: udp send failed: {e}");
                    }
                }
                Output::Event(event) => match event {
                    Event::Connected => {
                        tracing::info!("voice rtc: connected (ice+dtls established)");
                        return Ok(Some(RtcEvent::Connected));
                    }
                    Event::IceConnectionStateChange(state) => {
                        if state == IceConnectionState::Disconnected {
                            tracing::info!("voice rtc: ice disconnected");
                            return Ok(Some(RtcEvent::Disconnected));
                        }
                    }
                    _ => {}
                },
            }
        }
    }
}

fn local_outbound_ip(remote: SocketAddr) -> Result<std::net::IpAddr, ClientError> {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| ClientError::GatewayClosed(format!("voice rtc: probe bind failed: {e}")))?;
    probe.connect(remote).map_err(|e| ClientError::GatewayClosed(format!("voice rtc: probe connect failed: {e}")))?;
    Ok(probe.local_addr().map_err(|e| ClientError::GatewayClosed(format!("voice rtc: probe local_addr failed: {e}")))?.ip())
}

fn format_fingerprint(fp: &Fingerprint) -> String {
    fp.bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}
