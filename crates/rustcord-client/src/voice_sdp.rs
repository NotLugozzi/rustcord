pub struct LocalIceParams {
    pub ufrag: String,
    pub pwd: String,
    pub fingerprint: String,
}

pub fn build_offer(ice: &LocalIceParams) -> String {
    format!(
        "a=extmap-allow-mixed\n\
         a=ice-ufrag:{}\n\
         a=ice-pwd:{}\n\
         a=ice-options:trickle\n\
         a=fingerprint:sha-256 {}\n\
         a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level\n\
         a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\n\
         a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\n\
         a=rtpmap:111 opus/48000/2\n\
         a=extmap:14 urn:ietf:params:rtp-hdrext:toffset\n\
         a=extmap:13 urn:3gpp:video-orientation\n\
         a=extmap:5 http://www.webrtc.org/experiments/rtp-hdrext/playout-delay\n\
         a=extmap:6 http://www.webrtc.org/experiments/rtp-hdrext/video-content-type\n\
         a=extmap:7 http://www.webrtc.org/experiments/rtp-hdrext/video-timing\n\
         a=extmap:8 http://www.webrtc.org/experiments/rtp-hdrext/color-space\n\
         a=extmap:10 urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id\n\
         a=extmap:11 urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id\n\
         a=rtpmap:96 VP8/90000\n\
         a=rtpmap:97 rtx/90000\n",
        ice.ufrag, ice.pwd, ice.fingerprint
    )
}

#[derive(Debug, Clone)]
pub struct RemoteCandidate {
    pub ip: String,
    pub port: u16,
    pub transport: String,
}

#[derive(Debug, Clone)]
pub struct RemoteSdp {
    pub ice_ufrag: String,
    pub ice_pwd: String,
    pub fingerprint: String,
    pub candidates: Vec<RemoteCandidate>,
}

pub fn parse_remote_sdp(sdp: &str) -> Option<RemoteSdp> {
    let mut ice_ufrag = None;
    let mut ice_pwd = None;
    let mut fingerprint = None;
    let mut candidates = Vec::new();

    for line in sdp.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("a=ice-ufrag:") {
            ice_ufrag = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("a=ice-pwd:") {
            ice_pwd = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("a=fingerprint:") {
            fingerprint = value.split_once(' ').map(|(_, hash)| hash.to_string());
        } else if let Some(value) = line.strip_prefix("a=candidate:") {
            if let Some(candidate) = parse_candidate_line(value) {
                candidates.push(candidate);
            }
        }
    }

    Some(RemoteSdp { ice_ufrag: ice_ufrag?, ice_pwd: ice_pwd?, fingerprint: fingerprint?, candidates })
}

fn parse_candidate_line(value: &str) -> Option<RemoteCandidate> {
    let fields: Vec<&str> = value.split_whitespace().collect();
    if fields.len() < 5 {
        return None;
    }
    let transport = fields[2].to_lowercase();
    let ip = fields[4].to_string();
    let port = fields.get(5)?.parse().ok()?;
    Some(RemoteCandidate { ip, port, transport })
}
