//! Server configuration (TOML). Paths are resolved relative to the config file.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::netbin::{NetBinConfig, ServerConfig};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The user's own decrypted game executable; the cipher keys are read from it at startup.
    pub executable: PathBuf,
    /// Optional folder of extra static files served over HTTP (e.g. languages/english/motd.jpg). The
    /// generated config and texts are held in memory and never written here; they take precedence.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// The user's installed script archive: the one holding the compiled script modules, including
    /// `net.bin`, which is the base of the served config (see netbin::merge_module). Read at startup,
    /// never copied. See AGENTS.md for how to identify it.
    #[serde(default)]
    pub script_archive: Option<PathBuf>,
    /// The user's installed text archive: the one holding per-language text tables named
    /// `<n>.<category>`. Every `<n>.networking` in it is served, encrypted, as
    /// `build/main/text1/<n>.networking.crypt`, which the client downloads for its language index n.
    /// Read at startup, never copied. See AGENTS.md for how to identify it.
    #[serde(default)]
    pub text_archive: Option<PathBuf>,
    /// Replaces the served `<n>.networking.crypt` for a language index with a ready `.crypt` file.
    #[serde(default)]
    pub text_overrides: std::collections::BTreeMap<String, PathBuf>,
    /// Host name or IPv4 address the client should use for our UDP services (written into net.bin).
    pub public_host: String,
    pub http: HttpConfig,
    pub auth: AuthConfig,
    pub presence: PresenceConfig,
    #[serde(default)]
    pub netbin: NetBinOverrides,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    pub bind: SocketAddr,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub bind: SocketAddr,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceConfig {
    pub bind: SocketAddr,
    /// Seconds sent in "heartbeat next N"; the client ignores values below 30.
    #[serde(default = "default_interval")]
    pub heartbeat_interval_secs: u32,
    /// A session is dropped after this long without a message.
    #[serde(default = "default_timeout")]
    pub session_timeout_secs: u64,
    /// Coordinates returned for "get-location" (used by the client for geographic matchmaking).
    #[serde(default)]
    pub default_location: Location,
    /// The daily unique-player count (sent as "playercount") restarts at 00:00 in this UTC offset,
    /// e.g. 8 for UTC+8. Uses the system clock.
    #[serde(default)]
    pub daily_reset_utc_offset_hours: i32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub lat: f32,
    pub lon: f32,
}

impl Default for Location {
    fn default() -> Self {
        Self { lat: 0.0, lon: 0.0 }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetBinOverrides {
    /// Optional JSON with both server configs (see netbin::NetBinConfig); replaces the derived config.
    #[serde(default)]
    pub json: Option<PathBuf>,
}

fn default_interval() -> u32 {
    30
}
fn default_timeout() -> u64 {
    120
}

/// Queue paths written into the served config. The client posts reports to them on a queue service
/// whose host is fixed in the client; nothing here receives them, so the posts fail harmlessly.
///
/// TODO(stats queue): to collect match reports later:
///   1. Config: add a `[stats]` section (e.g. `queue = "..."`, storage path) and use it here instead
///      of these constants; keep the defaults so the served bytes don't change for existing users.
///   2. HTTP: the client sends `POST <queue path>` with a form body (`Action=SendMessage`,
///      `MessageBody=<base64 report>`, plus request-signing fields to ignore). `http::respond` rejects
///      every POST today; route queue POSTs to a new module (e.g. `stats.rs`) that stores the body
///      and answers with an SQS-style `SendMessageResponse` XML (200).
///   3. Decode: the match report is a base64 bitstream (online id, map, results); the TUS mirror
///      message is the online id padded to 16 bytes + a versioned player record. Both formats still
///      need documenting.
///   4. DNS: the queue service host must also be overridden to `public_host`. Document how to find
///      it in AGENTS.md the same way as the live-data hosts (plain-HTTP POSTs with `Action=SendMessage`).
const STATS_QUEUE: &str = "/uc2-stats";
const TUS_MIRROR_QUEUE: &str = "/uc2-tus-mirror";

/// public_host is written into net.bin and the client sends login and presence traffic to it, so it
/// must be an address the client can reach. A wildcard or a leftover placeholder would only show up
/// as a silent failure on the client, so it is rejected at startup.
fn check_public_host(host: &str) -> Result<()> {
    let h = host.trim();
    let wildcard = h
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_unspecified());
    if h.is_empty() || wildcard || h.contains("CHANGE") {
        anyhow::bail!(
            "public_host = {host:?}: set it to this machine's LAN address as the client sees it \
                       (e.g. 192.168.1.50); the bind addresses can stay 0.0.0.0"
        );
    }
    Ok(())
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        check_public_host(&cfg.public_host)?;
        let base = path.parent().unwrap_or(Path::new("."));
        cfg.executable = base.join(&cfg.executable);
        for p in [
            &mut cfg.data_dir,
            &mut cfg.netbin.json,
            &mut cfg.script_archive,
            &mut cfg.text_archive,
        ] {
            if let Some(rel) = p.take() {
                *p = Some(base.join(rel));
            }
        }
        for path in cfg.text_overrides.values_mut() {
            *path = base.join(&*path);
        }
        cfg.check_data_dir()?;
        Ok(cfg)
    }

    /// Everything in data_dir is served to anyone who can reach the HTTP port, so it must not hold
    /// the game files (above all the decrypted executable). Checked on the resolved real paths.
    fn check_data_dir(&self) -> Result<()> {
        let Some(configured) = self.data_dir.as_deref() else {
            return Ok(());
        };
        let Ok(dir) = std::fs::canonicalize(configured) else {
            return Ok(());
        };
        let files = [
            Some(&self.executable),
            self.script_archive.as_ref(),
            self.text_archive.as_ref(),
        ];
        for file in files.into_iter().flatten() {
            if std::fs::canonicalize(file).is_ok_and(|f| f.starts_with(&dir)) {
                anyhow::bail!(
                    "data_dir = {}: it contains {}, and everything in data_dir is served to anyone who can \
                               reach the HTTP port; use a folder that holds only the extra static files",
                    configured.display(),
                    file.display()
                );
            }
        }
        Ok(())
    }

    /// The installed net.bin from `script_archive`, if configured.
    pub fn base_net_module(&self) -> Result<Option<Vec<u8>>> {
        let Some(path) = &self.script_archive else {
            return Ok(None);
        };
        let blob = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        crate::netbin::read_archive_entry(&blob, "net.bin")
            .with_context(|| format!("script_archive = {}: not the script archive (it must be an archive \
                                      listing compiled script modules, including net.bin; see AGENTS.md)",
                                     path.display()))
            .map(Some)
    }

    /// The server configs: from the JSON override, or derived from public_host and our bind ports.
    pub fn netbin_config(&self) -> Result<NetBinConfig> {
        if let Some(path) = &self.netbin.json {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            return serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()));
        }
        let presence_port = self.presence.bind.port() as u32;
        let server = ServerConfig {
            host_a: self.public_host.clone(),
            port_a: presence_port,
            host_b: self.public_host.clone(),
            port_b: presence_port,
            auth_host: self.public_host.clone(),
            auth_port: self.auth.bind.port() as u32,
            auth_host2: self.public_host.clone(),
            stats_prefix: None,
            // See STATS_QUEUE for what these are and the TODO for collecting them.
            stats_queue: STATS_QUEUE.into(),
            stats_queues: vec![STATS_QUEUE.into()],
            tus_mirror_queue: TUS_MIRROR_QUEUE.into(),
            // Empty: the client then skips its NAT-type report (it only sends one when this is set).
            nat_queue: String::new(),
            // The client aborts its online start when the matchmaking world count summed over all
            // servers is below this value. The shipped value is 20; 1 only requires that matchmaking
            // exists at all.
            session_minimum: 1,
            net_game_param44: 12,
            param34: 32,
            param38: 64,
            tag40: 0,
        };
        Ok(NetBinConfig {
            config_a: server.clone(),
            config_b: server,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, check_public_host};

    /// The shipped example parses, and its written-out values equal the defaults.
    #[test]
    fn example_config_parses() {
        let text = include_str!("../uc2-server.example.toml").replace("CHANGE-ME", "192.168.1.50");
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(
            cfg.netbin_config().unwrap().config_a.stats_queue,
            "/uc2-stats"
        );
        assert_eq!(cfg.presence.heartbeat_interval_secs, 30);
        assert_eq!(cfg.presence.session_timeout_secs, 120);
        assert!(
            cfg.text_overrides.is_empty() && cfg.data_dir.is_none() && cfg.netbin.json.is_none()
        );
    }

    /// The minimal config (only the required keys) gets the documented defaults.
    #[test]
    fn minimal_config_uses_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            public_host = "192.168.1.50"
            executable = "game/a"
            script_archive = "game/b"
            text_archive = "game/c"
            [http]
            bind = "0.0.0.0:80"
            [auth]
            bind = "0.0.0.0:3233"
            [presence]
            bind = "0.0.0.0:3232"
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.netbin_config().unwrap().config_a.stats_queue,
            "/uc2-stats"
        );
        assert_eq!(cfg.presence.heartbeat_interval_secs, 30);
        assert_eq!(cfg.presence.session_timeout_secs, 120);
        assert_eq!(cfg.presence.daily_reset_utc_offset_hours, 0);
        assert_eq!(
            (
                cfg.presence.default_location.lat,
                cfg.presence.default_location.lon
            ),
            (0.0, 0.0)
        );
        assert!(cfg.text_overrides.is_empty() && cfg.data_dir.is_none());
    }

    /// A data_dir that holds a game file is refused; one beside the game folder is fine.
    #[test]
    fn data_dir_must_not_hold_the_game_files() {
        let dir = std::env::temp_dir().join(format!("uc2-config-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("game")).unwrap();
        std::fs::create_dir_all(dir.join("static")).unwrap();
        std::fs::write(dir.join("game/exe"), b"x").unwrap();
        let load = |data_dir: &str| {
            let text = format!(
                "public_host = \"192.168.1.50\"\nexecutable = \"game/exe\"\ndata_dir = \"{data_dir}\"\n\
                                [http]\nbind = \"0.0.0.0:80\"\n[auth]\nbind = \"0.0.0.0:3233\"\n\
                                [presence]\nbind = \"0.0.0.0:3232\"\n"
            );
            std::fs::write(dir.join("uc2-server.toml"), text).unwrap();
            Config::load(&dir.join("uc2-server.toml"))
        };
        let (root, game, separate) = (load("."), load("game"), load("static"));
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            root.is_err() && game.is_err(),
            "a folder holding the executable is refused"
        );
        assert!(separate.is_ok(), "{:?}", separate.err());
    }

    #[test]
    fn public_host_must_be_reachable() {
        assert!(check_public_host("192.168.1.50").is_ok());
        assert!(check_public_host("server.example.lan").is_ok());
        for bad in ["", "0.0.0.0", "::", "CHANGE-ME"] {
            assert!(check_public_host(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
