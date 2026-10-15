//! Login handshake ("ticket echo"), UDP. The client sends its login ticket in plaintext and accepts
//! the server once it receives encrypt_blob(ticket): it decrypts the reply and compares the first
//! ticket-length bytes with what it sent. Nothing else is in the reply.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{info, warn};

use crate::cipher::BlobCipher;
use crate::state::Registry;
use crate::ticket;

/// The client reads the reply into a 0x3ff-byte buffer and requires at least 20 bytes.
const CLIENT_RECV_MAX: usize = 0x3ff;

pub async fn serve(bind: std::net::SocketAddr, cipher: Arc<BlobCipher>, registry: Arc<Registry>) -> anyhow::Result<()> {
    let sock = UdpSocket::bind(bind).await?;
    info!(%bind, "auth (ticket echo) listening");
    let mut buf = vec![0u8; 0x2000];
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "auth recv failed");
                continue;
            }
        };
        let ticket_bytes = &buf[..n];
        let parsed = ticket::parse(ticket_bytes);
        match &parsed {
            Some(t) => info!(%peer, len = n, version = format_args!("{:#010x}", t.version),
                online_id = t.online_id.as_deref().unwrap_or("?"), issuer = ?t.issuer_id,
                service = t.service_id.as_deref().unwrap_or("?"), region = t.region.as_deref().unwrap_or("?"),
                "auth: ticket received"),
            None => warn!(%peer, len = n, "auth: datagram is not a parseable login ticket; echoing anyway"),
        }
        let reply = cipher.encrypt_blob(ticket_bytes);
        if reply.len() > CLIENT_RECV_MAX {
            warn!(%peer, reply = reply.len(), "auth: reply exceeds the client's 0x3ff-byte buffer; client will fail");
        }
        if let Err(e) = sock.send_to(&reply, peer).await {
            warn!(%peer, error = %e, "auth: send failed");
            continue;
        }
        let today = parsed.as_ref().and_then(|t| t.online_id.as_deref()).map(|id| registry.record_player(id));
        registry.record_ticket(peer.ip(), parsed);
        info!(%peer, reply = reply.len(), players_today = ?today, "auth: echoed");
    }
}
