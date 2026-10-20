//! Login handshake ("ticket echo"), UDP. The client sends its login ticket in plaintext and accepts
//! the server once it receives encrypt_blob(ticket): it decrypts the reply and compares the first
//! ticket-length bytes with what it sent. Nothing else is in the reply.
//!
//! Only a datagram that parses as a login ticket with a well-formed online id is answered, and only
//! with a reply the client can receive. Anything else gets no reply, so the port can't be used to
//! encrypt arbitrary data or to reflect traffic at a spoofed source address.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{info, warn};

use crate::cipher::BlobCipher;
use crate::state::{REJECTED_LOGS_PER_MINUTE, Registry, Throttle};
use crate::ticket;

/// The client reads the reply into a 0x3ff-byte buffer and requires at least 20 bytes.
const CLIENT_RECV_MAX: usize = 0x3ff;

pub async fn serve(
    bind: std::net::SocketAddr,
    cipher: Arc<BlobCipher>,
    registry: Arc<Registry>,
) -> anyhow::Result<()> {
    let sock = UdpSocket::bind(bind).await?;
    info!(%bind, "auth (ticket echo) listening");
    let mut buf = vec![0u8; 0x2000];
    let mut rejected = Throttle::new(REJECTED_LOGS_PER_MINUTE);
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                if let Some(suppressed) = rejected.allow() {
                    warn!(error = %e, suppressed, "auth recv failed");
                }
                continue;
            }
        };
        let ticket_bytes = &buf[..n];
        let parsed = ticket::parse(ticket_bytes)
            .filter(|t| t.online_id.as_deref().is_some_and(ticket::is_online_id));
        let Some(t) = parsed else {
            if let Some(suppressed) = rejected.allow() {
                warn!(%peer, len = n, suppressed, "auth: datagram is not a login ticket with a valid online id; ignored");
            }
            continue;
        };
        let reply = cipher.encrypt_blob(ticket_bytes);
        if reply.len() > CLIENT_RECV_MAX {
            if let Some(suppressed) = rejected.allow() {
                warn!(%peer, reply = reply.len(), suppressed,
                    "auth: reply would exceed the client's 0x3ff-byte buffer; not sent");
            }
            continue;
        }
        info!(%peer, len = n, version = format_args!("{:#010x}", t.version),
            online_id = t.online_id.as_deref().unwrap_or("?"), issuer = ?t.issuer_id,
            service = t.service_id.as_deref().unwrap_or("?"), region = t.region.as_deref().unwrap_or("?"),
            "auth: ticket received");
        if let Err(e) = sock.send_to(&reply, peer).await {
            if let Some(suppressed) = rejected.allow() {
                warn!(%peer, error = %e, suppressed, "auth: send failed");
            }
            continue;
        }
        let today = t.online_id.as_deref().map(|id| registry.record_player(id));
        registry.record_ticket(peer.ip());
        info!(%peer, reply = reply.len(), players_today = ?today, "auth: echoed");
    }
}
