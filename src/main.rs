//! uc2-server: a community server for the online mode of UC2. Not affiliated with or
//! endorsed by the game's developer or publisher.
//!   * live-data HTTP: the online config module and networking texts, generated at startup from the
//!     user's own game files and held in memory only; plus optional static files from data_dir
//!   * login handshake (ticket echo, UDP)
//!   * presence: heartbeat / player count / location (UDP)
//! Platform services (sign-in, matchmaking, stats storage) are not provided here.

mod auth;
mod cipher;
mod config;
mod elf;
mod http;
mod netbin;
mod presence;
mod state;
mod ticket;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{info, warn};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run all services.
    Run {
        #[arg(short, long, default_value = "uc2-server.toml")]
        config: PathBuf,
    },
    /// Write net.bin, net.bin.archive and net.bin.archive.crypt into a directory for inspection. The
    /// output is derived from the user's own game files: keep it local.
    GenNetbin {
        #[arg(short, long, default_value = "uc2-server.toml")]
        config: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },
    /// List the entries of a game archive and say whether it fits `script_archive` or
    /// `text_archive`. Reads the file only; writes nothing.
    ListArchive {
        /// The archive to inspect.
        file: PathBuf,
        /// Print every entry instead of a sample.
        #[arg(long)]
        all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match Cli::parse().cmd {
        Cmd::Run { config } => run(&config).await,
        Cmd::GenNetbin { config, out } => {
            let cfg = config::Config::load(&config)?;
            let cipher = cipher::BlobCipher::from_executable(&cfg.executable)?;
            let a = netbin::build_all(&cfg.netbin_config()?, cfg.base_net_module()?.as_deref(), &cipher)?;
            std::fs::create_dir_all(&out)?;
            std::fs::write(out.join("net.bin"), &a.module)?;
            std::fs::write(out.join("net.bin.archive"), &a.archive)?;
            std::fs::write(out.join("net.bin.archive.crypt"), &a.crypt)?;
            println!("net.bin {} bytes, archive {}, crypt {} -> {}", a.module.len(), a.archive.len(), a.crypt.len(), out.display());
            Ok(())
        }
        Cmd::ListArchive { file, all } => list_archive(&file, all),
    }
}

/// What an archive's entry names say about its role for this server.
fn archive_role(names: &[String]) -> &'static str {
    if names.iter().any(|n| n == "net.bin") {
        "fits script_archive (lists net.bin among its script modules)"
    } else if names.iter().any(|n| n.split_once('.').is_some_and(|(i, c)| c == "networking" && i.parse::<u32>().is_ok())) {
        "fits text_archive (per-language <n>.<category> tables, including <n>.networking)"
    } else {
        "fits neither script_archive nor text_archive"
    }
}

fn list_archive(file: &Path, all: bool) -> Result<()> {
    let blob = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let names = netbin::archive_manifest(&blob).with_context(|| format!("{} is not a readable game archive", file.display()))?;
    println!("{}: {} entries", file.display(), names.len());
    let shown = if all { names.len() } else { names.len().min(20) };
    for n in &names[..shown] {
        println!("  {n}");
    }
    if shown < names.len() {
        println!("  ... {} more (use --all)", names.len() - shown);
    }
    println!("=> {}", archive_role(&names));
    Ok(())
}

/// `build/main/text1/<n>.networking.crypt`, which the client downloads for its language index n; a
/// missing file aborts the online start ("error syncing text"). Each comes from the user's text
/// archive unless text_overrides names a ready .crypt for that index.
fn networking_texts(cfg: &config::Config, cipher: &cipher::BlobCipher, assets: &mut http::Assets) -> Result<()> {
    let key = |index: &str| format!("build/main/text1/{index}.networking.crypt");
    let mut written = Vec::new();
    if let Some(path) = &cfg.text_archive {
        let blob = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let names = netbin::archive_manifest(&blob)
            .with_context(|| format!("text_archive = {}: not a readable archive (see AGENTS.md)", path.display()))?;
        for name in names {
            let Some(index) = name.strip_suffix(".networking") else { continue };
            if cfg.text_overrides.contains_key(index) {
                continue;
            }
            let text = netbin::read_archive_entry(&blob, &name)?;
            assets.insert(key(index), http::Asset::new(cipher.encrypt_stream(&text)));
            written.push(index.to_string());
        }
        if written.is_empty() && cfg.text_overrides.is_empty() {
            anyhow::bail!("text_archive = {}: not the text archive (it has no <n>.networking entries; see AGENTS.md)",
                          path.display());
        }
    }
    for (index, source) in &cfg.text_overrides {
        let bytes = std::fs::read(source).with_context(|| format!("reading text override {}", source.display()))?;
        assets.insert(key(index), http::Asset::new(bytes));
        info!(index = %index, source = %source.display(), "networking text override");
    }
    if written.is_empty() && cfg.text_overrides.is_empty() {
        warn!("neither text_archive nor text_overrides is set: build/main/text1/<n>.networking.crypt will 404 \
               and the client stops with a text sync error");
    } else {
        info!(from_game = %written.join(","), overrides = cfg.text_overrides.len(), "networking texts ready");
    }
    Ok(())
}

async fn run(config_path: &Path) -> Result<()> {
    let cfg = config::Config::load(config_path)?;
    let cipher = Arc::new(cipher::BlobCipher::from_executable(&cfg.executable)?);
    info!(executable = %cfg.executable.display(), "cipher keys loaded");

    let netbin_cfg = cfg.netbin_config()?;
    let base_module = cfg.base_net_module()?;
    match &base_module {
        Some(m) => info!(bytes = m.len(), "using the installed net.bin as the base of the served config"),
        None => warn!("script_archive is not set: the served net.bin holds only the server configs and drops \
                       the other online settings; the client then stalls during its online start"),
    }
    let artifacts = netbin::build_all(&netbin_cfg, base_module.as_deref(), &cipher)?;
    let mut assets = http::Assets::new();
    // Each requested name gets an archive whose entry has that same name; net-patch9 also carries a
    // net.bin entry for earlier builds.
    for name in netbin::REQUESTED_NAMES {
        let mut entries = vec![(name, artifacts.module.as_slice())];
        if name != "net.bin" {
            entries.push(("net.bin", artifacts.module.as_slice()));
        }
        let asset = http::Asset::new(cipher.encrypt_stream(&netbin::build_archive(&entries)));
        info!(path = %format!("/{name}.psarc.crypt"), bytes = asset.len(), "live-data config ready (in memory)");
        assets.insert(format!("{name}.psarc.crypt"), asset);
    }
    info!(auth = format_args!("{}:{}", netbin_cfg.config_a.auth_host, netbin_cfg.config_a.auth_port),
        presence_port = netbin_cfg.config_a.port_a, "net.bin points the client here");
    networking_texts(&cfg, &cipher, &mut assets)?;

    let registry = Arc::new(state::Registry::new(cfg.presence.daily_reset_utc_offset_hours));
    let http = tokio::spawn(http::serve(cfg.http.bind, assets, cfg.data_dir.clone()));
    let auth = tokio::spawn(auth::serve(cfg.auth.bind, cipher.clone(), registry.clone()));
    let presence = tokio::spawn(presence::serve(cfg.presence, cipher.clone(), registry.clone()));

    tokio::select! {
        r = http => r??,
        r = auth => r??,
        r = presence => r??,
        _ = tokio::signal::ctrl_c() => info!("shutting down"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::archive_role;

    #[test]
    fn archive_roles_follow_entry_names() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(archive_role(&names(&["alpha.bin", "net.bin", "beta.bin"])).starts_with("fits script_archive"));
        assert!(archive_role(&names(&["2.common", "2.networking", "3.subtitles"])).starts_with("fits text_archive"));
        assert!(archive_role(&names(&["2.common", "x.networking"])).starts_with("fits neither"));
        assert!(archive_role(&names(&["map.pak"])).starts_with("fits neither"));
    }
}
