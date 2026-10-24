use crate::error::ClientError;
use tokio::net::UdpSocket;

pub struct Discovered {
    pub external_ip: String,
    pub external_port: u16,
}

pub async fn discover(socket: &UdpSocket, ssrc: u32) -> Result<Discovered, ClientError> {
    let mut packet = [0u8; 74];
    packet[0..2].copy_from_slice(&1u16.to_be_bytes());
    packet[2..4].copy_from_slice(&70u16.to_be_bytes());
    packet[4..8].copy_from_slice(&ssrc.to_be_bytes());

    socket.send(&packet).await.map_err(|e| ClientError::GatewayClosed(format!("voice udp send failed: {e}")))?;

    let mut response = [0u8; 74];
    let len = socket
        .recv(&mut response)
        .await
        .map_err(|e| ClientError::GatewayClosed(format!("voice udp recv failed: {e}")))?;
    if len < 74 {
        return Err(ClientError::GatewayClosed("voice udp discovery response too short".into()));
    }

    let ip_end = response[8..].iter().position(|&b| b == 0).map(|p| 8 + p).unwrap_or(72);
    let external_ip = String::from_utf8_lossy(&response[8..ip_end]).to_string();
    let external_port = u16::from_be_bytes([response[72], response[73]]);

    Ok(Discovered { external_ip, external_port })
}
