//! Presence service (UDP): cipher blobs carrying space-separated ASCII.
//!
//! Client -> server (first token is the player's online id):
//!   <id> heartbeat <playing|menu> headset <0|1> party <0|1> match_mode <n> gametype <n> ...
//!   <id> playercount
//!   <id> get-location disc <s> hdsd <0|1> up <n> down <n> wired <0|1>
//! Server -> client (tokenized on single spaces):
//!   heartbeat next <secs>     next heartbeat in max(secs, 30) s; second token must be "next"
//!   playercount <n>           the client stores n + 1
//!   location <lat> <lon>      the client's coordinates; used for geographic matchmaking

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::cipher::BlobCipher;
use crate::config::PresenceConfig;
use crate::state::Registry;

/// The client receives replies into a 0xff-byte buffer.
const CLIENT_RECV_MAX: usize = 0xff;

#[derive(Debug, PartialEq)]
pub enum Request<'a> {
    Heartbeat { id: &'a str, state: &'a str, headset: bool, party: bool, match_mode: i64, game_type: i64 },
    PlayerCount { id: &'a str },
    GetLocation { id: &'a str, disc: &'a str, hd: bool, up: i64, down: i64, wired: bool },
}

/// Parses a decrypted request. Keyword/value pairs are matched by name so field order changes or
/// extra fields do not break parsing.
pub fn parse(text: &str) -> Option<Request<'_>> {
    let t: Vec<&str> = text.split_whitespace().collect();
    let (id, cmd) = (*t.first()?, *t.get(1)?);
    let kv = |key: &str| t.iter().position(|w| *w == key).and_then(|i| t.get(i + 1).copied());
    let num = |key: &str| kv(key).and_then(|v| v.parse::<i64>().ok());
    match cmd {
        "heartbeat" => Some(Request::Heartbeat {
            id,
            state: t.get(2).copied().unwrap_or("unknown"),
            headset: num("headset").unwrap_or(0) != 0,
            party: num("party").unwrap_or(0) != 0,
            match_mode: num("match_mode").unwrap_or(0),
            game_type: num("gametype").unwrap_or(-1),
        }),
        "playercount" => Some(Request::PlayerCount { id }),
        "get-location" => Some(Request::GetLocation {
            id,
            disc: kv("disc").unwrap_or(""),
            hd: num("hdsd").unwrap_or(0) != 0,
            up: num("up").unwrap_or(0),
            down: num("down").unwrap_or(0),
            wired: num("wired").unwrap_or(0) != 0,
        }),
        _ => None,
    }
}

/// Builds the reply text for a request (without the trailing NUL).
/// `players_today`: unique players seen today, including the requester (state::DailyPlayers; the
/// set is cleared when the day changes, see presence.daily_reset_utc_offset_hours).
pub fn reply(req: &Request<'_>, cfg: &PresenceConfig, players_today: usize) -> String {
    match req {
        Request::Heartbeat { .. } => format!("heartbeat next {}", cfg.heartbeat_interval_secs.max(30)),
        // The client adds 1 to the received value; we assume it counts itself, so send the number
        // of *other* players today (an assumption: the original server's convention is unknown).
        // TODO(player count): the convention is unverified; with one player the menu shows 0. To finish:
        //   1. With two consoles online, compare the menu's number with the server's
        //      `players_today` log value (auth: echoed).
        //   2. Find where the client handles the playercount reply and which menu reads the stored
        //      value, to learn whether the shown value is n, n + 1, or something else (e.g. per playlist).
        //   3. Change the formula here and the replies_match_client_parser test together.
        Request::PlayerCount { .. } => format!("playercount {}", players_today.saturating_sub(1)),
        Request::GetLocation { .. } => {
            format!("location {:.4} {:.4}", cfg.default_location.lat, cfg.default_location.lon)
        }
    }
}

pub async fn serve(cfg: PresenceConfig, cipher: Arc<BlobCipher>, registry: Arc<Registry>) -> anyhow::Result<()> {
    let sock = UdpSocket::bind(cfg.bind).await?;
    info!(bind = %cfg.bind, "presence listening");
    let timeout = Duration::from_secs(cfg.session_timeout_secs);
    let pruner = registry.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            for id in pruner.prune(timeout) {
                info!(online_id = %id, "presence: session expired");
            }
        }
    });

    let mut buf = vec![0u8; 0x2000];
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "presence recv failed");
                continue;
            }
        };
        let Some(plain) = cipher.decrypt_blob(&buf[..n]) else {
            warn!(%peer, len = n, "presence: datagram failed cipher verification");
            continue;
        };
        let end = plain.iter().position(|&b| b == 0).unwrap_or(plain.len());
        let text = String::from_utf8_lossy(&plain[..end]).into_owned();
        let Some(req) = parse(&text) else {
            warn!(%peer, %text, "presence: unrecognized request");
            continue;
        };
        handle(&sock, &cfg, &cipher, &registry, peer, &text, &req).await;
    }
}

async fn handle(sock: &UdpSocket, cfg: &PresenceConfig, cipher: &BlobCipher, registry: &Registry, peer: SocketAddr,
                text: &str, req: &Request<'_>) {
    let id = match req {
        Request::Heartbeat { id, .. } | Request::PlayerCount { id } | Request::GetLocation { id, .. } => *id,
    };
    if !registry.has_ticket(peer.ip()) {
        debug!(%peer, online_id = id, "presence: no login handshake seen from this address");
    }
    let new = registry.touch(id, peer, |s| {
        if let Request::Heartbeat { state, headset, party, match_mode, game_type, .. } = req {
            s.state = state.to_string();
            s.headset = *headset;
            s.party = *party;
            s.match_mode = *match_mode;
            s.game_type = *game_type;
        }
    });
    if new {
        info!(%peer, online_id = id, "presence: new session");
    }
    // Also recorded here, so players already online when the server restarts are counted.
    let today = registry.record_player(id);
    let out = reply(req, cfg, today);
    let mut plain = out.clone().into_bytes();
    plain.push(0); // NUL-terminate: the client tokenizes the decrypted buffer as a C string
    let blob = cipher.encrypt_blob(&plain);
    if blob.len() > CLIENT_RECV_MAX {
        warn!(%peer, len = blob.len(), "presence: reply exceeds the client's 0xff-byte buffer");
    }
    match sock.send_to(&blob, peer).await {
        Ok(_) => info!(%peer, request = %text, reply = %out, "presence"),
        Err(e) => warn!(%peer, error = %e, "presence: send failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Location;

    fn cfg() -> PresenceConfig {
        PresenceConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            heartbeat_interval_secs: 45,
            session_timeout_secs: 120,
            default_location: Location { lat: 34.0542, lon: -118.2411 },
            daily_reset_utc_offset_hours: 0,
        }
    }

    #[test]
    fn parses_client_formats() {
        // Note the double space after the state in heartbeat, as the client sends it.
        assert_eq!(
            parse("Player1 heartbeat playing  headset 1 party 0 match_mode 3 gametype 7"),
            Some(Request::Heartbeat { id: "Player1", state: "playing", headset: true, party: false, match_mode: 3, game_type: 7 })
        );
        assert_eq!(parse("Player1 playercount"), Some(Request::PlayerCount { id: "Player1" }));
        assert_eq!(
            parse("Player1 get-location disc XXXX00000 hdsd 1 up 1200 down 8000 wired 1"),
            Some(Request::GetLocation { id: "Player1", disc: "XXXX00000", hd: true, up: 1200, down: 8000, wired: true })
        );
        assert_eq!(parse("Player1 bogus"), None);
    }

    #[test]
    fn replies_match_client_parser() {
        let c = cfg();
        assert_eq!(reply(&Request::PlayerCount { id: "x" }, &c, 5), "playercount 4");
        assert_eq!(
            reply(&Request::Heartbeat { id: "x", state: "menu", headset: false, party: false, match_mode: 0, game_type: -1 }, &c, 1),
            "heartbeat next 45"
        );
        assert_eq!(
            reply(&Request::GetLocation { id: "x", disc: "", hd: false, up: 0, down: 0, wired: false }, &c, 1),
            "location 34.0542 -118.2411"
        );
    }
}
