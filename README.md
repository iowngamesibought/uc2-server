# uc2-server

A community server for the online mode of **UC2** on certain game console.

This project is not affiliated with or endorsed by the game's developer or publisher. It ships no game files: the server reads what it needs from **your own installed copy** at startup and keeps everything it derives from it in memory only.

## Required: a DNS override

The console only reaches this server if **two specific domains** that the game contacts resolve to this machine. Without that override the console keeps trying the original hosts, which no longer serve the game, and nothing arrives here.

This README deliberately doesn't name the domains. To find them, **ask an AI agent to follow [AGENTS.md](AGENTS.md)**: it identifies both names from publicly shared packet captures of the game and from your own executable, then walks you through the override (router or dnsmasq, proxy exemptions) and checks that it works. Once you have the names, the setup itself is two lines (see [Network setup](#network-setup)).

## Required: three files from your copy of the game

The server ships no game data. It needs three files from **your own installed copy**, set in the config:

| Config key | What it is |
|---|---|
| `executable` | The game's main executable, decrypted from your own installation. The server reads its cipher keys from it at startup. |
| `script_archive` | The archive of compiled script modules. The online config the console downloads is built from it. |
| `text_archive` | The archive of per-language text tables. The console's online text comes from it. |

This README deliberately doesn't name the files. To locate them, **ask an AI agent to follow [AGENTS.md](AGENTS.md)**: it describes how to recognise each one in your install, and how to check its choice with the server (`uc2-server list-archive <file>` and the startup log). Never download these files from the internet; use only your own copy.

**Where to put them:** copy the three files into a `game/` folder next to your `uc2-server.toml`, and point the three keys at them:

```
uc2-server.toml
game/
  <main executable, decrypted>
  <script archive>
  <text archive>
```
```toml
executable     = "game/<main executable file name>"
script_archive = "game/<script archive file name>"
text_archive   = "game/<text archive file name>"
```

Relative paths are resolved from the config file's folder, so the files can also live anywhere else if you give absolute paths. Keep the original file names; they're only needed in your config. `game/` and your `uc2-server.toml` are listed in this project's `.gitignore`, so they won't be committed by accident.

## Run

```
cp uc2-server.example.toml uc2-server.toml    # set public_host and the three files above
cargo run --release -- run --config uc2-server.toml
```

`RUST_LOG=debug` gives more detail. Other commands:
- `list-archive <file>`: shows an archive's entries and whether it fits `script_archive` or `text_archive`.
- `gen-netbin --out <dir>`: writes the generated config to a folder for inspection. That output is derived from your game files, so keep it local.

## Network setup

- **DNS:** map the game's two live-data host names to this machine, e.g. with dnsmasq:
  ```
  address=/<config host>/<public_host>
  address=/<content host>/<public_host>
  ```
  To find the two names, see [Required: a DNS override](#required-a-dns-override). The login and presence address comes from `public_host` inside the served config, so no other names are needed. If a proxy or TUN sits in front of the console, exempt both names from fake-IP and route them directly.
- **public_host:** this machine's LAN address as the console sees it. The server refuses to start with a placeholder or `0.0.0.0`; the bind addresses can stay `0.0.0.0`.
- **Firewall:** allow inbound TCP 80 and UDP 3232/3233.
- **Ports:** the console uses the same port numbers locally, so run one server per LAN, and not on the same host as an emulator running the game.
- **Cache:** the console caches the config keyed on `Last-Modified`. The server derives that date from the content, so restarts don't force a re-download, but any config change does.

## Tests

`cargo test` covers the cipher (standard Blowfish vector, round trips), the config module (golden bytes, merging), archive round trips, client message formats, the daily player count, HTTP headers and paths, and config validation.

## Open questions

- `playercount`: the client adds 1 to the value; we send today's unique players minus one. Unverified with several consoles (see the TODO in `presence.rs`).
- `location`: a single configured coordinate, so everyone is in one matchmaking region.
