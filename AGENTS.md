# Instructions for agents helping a user set up uc2-server

You are helping someone run this server for their own copy of the game. Work through the steps below in order, and check each one before moving on. Read `README.md` first for what the server does.

## Ground rules

- **Use only the user's own copy of the game.** Never download game files, decrypted executables, keys or license files from the internet, and never suggest where to get them. If the user doesn't have the files, stop and tell them what's missing.
- **Never put game-derived files in this repository** (executables, archives, generated output, captures). Keep them in a folder outside it, or in a local `game/` folder the user doesn't commit.
- **Public packet captures are for finding host names only.** Don't copy anything else out of them: no file bodies, no other players' online IDs, no tickets.
- Run commands only with the user's agreement when they change their system (firewall rules, DNS, router settings).

## 1. Supply the required files

The server needs three files from the user's own installed copy. This guide deliberately doesn't name them: identify them from the descriptions below. They go into `uc2-server.toml` (copy it from `uc2-server.example.toml`) under `executable`, `script_archive` and `text_archive`; relative paths resolve from the config file's folder.

### Where to look

Ask the user where their copy of the installed game is and which platform it's for; they can give you the folder and that context. Within it, the files you need sit in the game's **data folder**: the one that holds the main executable and a `build/main/` tree of archives.

Copy the three files into a `game/` folder next to the user's `uc2-server.toml` (git-ignored in this project), keeping their original names, and set `executable`, `script_archive` and `text_archive` to `game/<file name>`. Absolute paths elsewhere work too. Never place them anywhere that gets committed.

### What each file is

| Config key | How to recognise it |
|---|---|
| `executable` | The game's **main boot executable**: the one file directly in the data folder with a `.BIN` extension, about 14–15 MB. As installed it is **encrypted** (it does not start with `\x7fELF`). The server needs it **decrypted** to an ELF: starts with `\x7fELF`, then bytes `02 02` (64-bit, big-endian). The user must provide that decrypted form from their own copy; it is their responsibility. Don't explain how to decrypt it, and never obtain a decrypted copy elsewhere. |
| `script_archive` | An **archive** in the game's own archive format, a few MB in `build/main/`. Its entries are compiled **script modules** with names like `<subsystem>.bin`, about 180 of them, and **one is `net.bin`**. |
| `text_archive` | An **archive** in the same format, about 2 MB in the same folder. Its entries are **per-language text tables** named `<number>.<category>` (e.g. `2.common`, `2.subtitles`), and the list includes **`<number>.networking`** for each language. |

`build/main/` holds several archives, including very large ones (hundreds of MB to GB) with map and character data. Those are never the right ones. Neither is a big patch archive directly in the data folder whose entries are full paths: it can contain text tables too, but not in the form the server needs.

### Check each candidate with the server itself

- `uc2-server list-archive <file>` lists an archive's entries and ends with a verdict: `fits script_archive`, `fits text_archive` or `fits neither`. It only reads the file. Try the candidates in `build/main/` until you have one of each.
- For the executable, start the server (next step). The startup log tells you whether it's right.

**Check:**
1. Set `public_host` to the server machine's LAN address as the console sees it (not `0.0.0.0`, not a placeholder).
2. Run `cargo run --release -- run --config uc2-server.toml` (or the packaged binary).
3. The log must show:
   - `cipher keys found build=...`. Errors and what they mean:
     - **"not a big-endian ELF64"**: the executable is still encrypted, or it's the wrong file.
     - **"N known key locations match"**: it's not the main executable of a supported build (GOTY 01.10 or one earlier build).
   - `using the installed net.bin as the base of the served config`. A **"not the script archive"** error means `script_archive` points at the wrong archive.
   - `networking texts ready from_game=...` listing language indexes. A **"not the text archive"** error means `text_archive` is wrong.
   - `live-data HTTP listening`, `auth (ticket echo) listening` and `presence listening`. A bind error usually means port 80 is in use (e.g. IIS) or another server instance is running.

## 2. Find the host names for the DNS override

This guide deliberately doesn't name the hosts. Work them out from publicly shared packet captures of the game's online mode (community collections of console network captures often include it), plus the user's own executable. Write only the two host names down; don't commit the capture or copy anything else from it.

There are **two** names, and both must resolve to `public_host`. The server answers for any `Host` header, so only DNS needs changing.

### What the traffic looks like

- **Plain HTTP over TCP port 80.** No TLS. The client sends the `Host` header and uses an HTTP-library user agent rather than a browser one.
- **When:** during the online start. After the console's own network test traffic and before or around the login handshake (UDP to port 3233), then again right after the platform's matchmaking connections.
- **The config host** looks like a **cloud object-storage bucket addressed by host name**: `<bucket name>.<storage provider's domain>`. The bucket name is a short dotted name with a game-specific prefix and words like "final"/"prod". Its responses come from that storage provider: `Server` header, request-id style headers, `ETag`, `Last-Modified`, `Content-Type: application/octet-stream`.
- **Requests to the config host** follow these patterns (paths only; any of them identifies the host):
  - `/<name>.bin.psarc.crypt` at the root: the online config archive, fetched first.
  - `/build/main/text1/<n>.networking.crypt`: per-language text; `n` is a small number.
  - `/build/main/pak22/<…>.pak.psarc.crypt`: map data packs. There may be many, and they're often cut off mid-body (the client only reads the date).
  - `/userdata/<online id>.txt.crypt`: usually answered 403 or 404.
- Everything the config host serves ends in **`.crypt`**: encrypted bodies that look random, with a 24-byte header starting with the big-endian plaintext length.
- **The content host** has the **same bucket name** under the **game developer's own domain**. The client only contacts it for optional downloads, so it is often **absent from captures**.
- **Not these:** the platform vendor's own domains (network-speed tests, profile and avatar images, trophy data, title resource XMLs, matchmaking on ports 3478/443) and message-queue POSTs to a queue service domain. They stay as they are.

### How to find them

1. **Config host from a capture:** pick one of the game connecting to multiplayer (before the official shutdown is best) and list the plain-HTTP requests:
   ```
   tshark -r <capture> -Y "http.request" -T fields -e http.host -e http.request.uri
   ```
   Take the host whose paths match the patterns above. If the capture has no HTTP requests (e.g. it only holds encrypted traffic), check its DNS answers (`-Y dns -e dns.qry.name`) for a name of the bucket shape described above.
2. **Content host from the user's executable:**
   - `strings -n 8 <executable> | grep "http://%s"` shows URL formats of the form `http://%s.<domain>`. One domain is the config host's; the other belongs to the content host.
   - The executable also holds several candidate bucket names (development, beta, production). Use the one the config host from step 1 starts with.
   - Content host = that bucket name + `.` + the other format's domain.
3. **Sanity checks:**
   - The config host = bucket name + `.` + the first format's domain. If it doesn't fit, you've picked the wrong host or the wrong format.
   - Neither name belongs to the platform vendor.

## 3. Set up the DNS override

Map both names to `public_host` on the DNS server the console uses. For dnsmasq (e.g. on an OpenWrt router):
```
address=/<config host>/<public_host>
address=/<content host>/<public_host>
```
Other options: a local DNS server, or the router's static-host setting. Then set the console to that DNS server, either automatically via DHCP or manually in its network settings.

**Proxies:** if a proxy or TUN (fake-IP) sits between the console and the internet, add both names to its fake-IP filter and route them DIRECT. Otherwise the proxy answers with its own addresses and the console never reaches the server.

**Check:**
1. `nslookup <config host> <console's DNS server>` must return `public_host`; the same for the content host. Query the console's DNS server explicitly: the machine you run the check on may use a different resolver.
2. Allow inbound TCP 80 and UDP 3232–3233 on the server machine (the Windows package has `allow-firewall.cmd`).
3. Start the game's online mode. The server log should show, in order:
   - `GET /net-patch9.bin.psarc.crypt 200` (twice, when the config changed since the console last downloaded it);
   - `auth: ticket received` and `auth: echoed`;
   - `GET /build/main/text1/<n>.networking.crypt 200`;
   - `GET /userdata/... 404` (normal);
   - `presence ... heartbeat`.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| No request reaches the server | DNS still resolves the config host elsewhere (check against the console's DNS server), a proxy intercepts it, or the firewall blocks port 80 |
| Config downloaded but no auth line | `public_host` isn't reachable from the console, or UDP 3233 is blocked |
| Console stuck on "connecting" after auth, no further requests | `script_archive` missing, or the console is using an old cached config; the server must send `Last-Modified` (it does by default) |
| "Error syncing text" | The text for the console's language index wasn't served: check `text_archive` and the `networking texts ready` log line |
| Only one `GET /net-patch9...` per start, and changes have no effect | The console's cached copy is considered current. Any change to the served config changes its date and forces a download. |
