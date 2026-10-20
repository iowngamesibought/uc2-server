# Instructions for agents helping a user set up uc2-server

You are helping someone run this server for their own copy of the game. Work through the steps below in order, and check each one before moving on. Read `README.md` first for what the server does.

## Ground rules

- **Use only the user's own copy of the game.** Never download game files, decrypted executables, keys or license files from the internet, and never suggest where to get them. If the user doesn't have the files, stop and tell them what's missing.
- **Never put game-derived files in this repository** (executables, archives, generated output, decompiler projects or exports, server logs). Keep them in a folder outside it, or in a local `game/` folder the user doesn't commit.
- **The host names you find in step 2 go into the user's DNS setup only.** Don't write them into this repository (code, docs, config examples, commits or issues), and don't copy anything else out of the executable: no keys, code or text. The server reads what it needs from the user's files at runtime.
- Run commands only with the user's agreement when they change their system (firewall rules, DNS, router settings).

## 1. Supply the required files

The server needs three files from the user's own installed copy. This guide deliberately doesn't name them: identify them from the descriptions below. They go into `uc2-server.toml` (copy it from `uc2-server.example.toml`) under `executable`, `script_archive` and `text_archive`; relative paths resolve from the config file's folder.

### Where to look

Ask the user where their copy of the installed game is and which platform it's for; they can give you the folder and that context. Copying the files off the console is the user's job, like decrypting the executable below; ask them for the files once they have them on this machine. Within it, the files you need sit in the game's **data folder**: the one that holds the main executable and a `build/main/` tree of archives.

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
2. Run `cargo run --release -- run --config uc2-server.toml` from this repository. There is no prebuilt binary: if `cargo` is missing, install the Rust toolchain first (rustup, https://rustup.rs).
3. The log must show:
   - `cipher keys found build=...`. Errors and what they mean:
     - **"not a big-endian ELF64"**: the executable is still encrypted, or it's the wrong file.
     - **"N known key locations match"**: it's not the main executable of a supported build (GOTY 01.10 or one earlier build).
   - `using the installed net.bin as the base of the served config`. A **"not the script archive"** error means `script_archive` points at the wrong archive.
   - `networking texts ready from_game=...` listing language indexes. A **"not the text archive"** error means `text_archive` is wrong.
   - `live-data HTTP listening`, `auth (ticket echo) listening` and `presence listening`. A bind error usually means port 80 is in use (e.g. IIS) or another server instance is running.

## 2. Find the host names in the user's executable

This guide deliberately doesn't name the hosts. Work them out from the **user's own decrypted executable** (the file set as `executable`); no packet captures are needed. Write only the host names down, and don't commit them or anything else from the executable.

There are **two** names, and both must resolve to `public_host`. The server answers for any `Host` header, so only DNS needs changing.

### What the client does

- It downloads its online config and its per-language texts over **plain HTTP** (port 80, no TLS) from a **cloud object-storage bucket addressed by host name**: `<bucket name>.<storage provider's domain>`. This is the **config host**. Requests look like `/<name>.bin.psarc.crypt` (the online config archive, fetched first), `/build/main/text1/<n>.networking.crypt`, `/build/main/pak22/<…>.pak.psarc.crypt` (map data; the client only reads the date) and `/userdata/<online id>.txt.crypt`.
- It builds a second base address from the same bucket name under the **game developer's own domain** (the **content host**), used only for optional downloads. Map it too; it is harmless when unused.
- Every other address the client uses (the login and presence ports) comes from the config this server generates, so no other names are needed.

### How to find them

List the executable's strings with `strings -n 6 <executable>` (Linux, macOS), or this Python equivalent (any platform; it only reads the file):
```
python -I -c "import re,sys;[print(m.group().decode()) for m in re.finditer(rb'[\x20-\x7e]{6,}',open(sys.argv[1],'rb').read())]" <executable>
```
Pipe the output into `grep`/`findstr` rather than saving it, or save it outside the repository.

1. **The two domains:** the strings starting with `http://%s.` are URL formats: there are exactly two, `http://%s.<domain>`. To tell them apart:
   - The **storage provider's domain** also appears elsewhere on its own: as a bare string (exactly the domain) and as `%s.<domain>` without the scheme.
   - The **developer's domain** appears in e-mail addresses instead, never on its own.
2. **The bucket name:** look for short dotted names ending in `.dev` or `.prod` (pattern `^[a-z0-9]+(\.[a-z0-9]+)*\.(dev|prod)$`). There are two pairs with the same game-specific prefix: a "beta" pair and a "final" pair. The live-data bucket is the **"final" one ending in `.prod`**: retail accounts use production, and the beta buckets belong to the game's beta. Don't go by length: the beta name is the shorter one.
3. **Compose the names:**
   - config host = `<bucket>.<storage provider's domain>`
   - content host = `<bucket>.<developer's domain>`
4. **Sanity checks:**
   - Both names start with the same bucket name; only the domain differs.
   - Neither name belongs to the console platform's own vendor.
   - Don't override anything else: not the message-queue service the client posts match reports to, and not the platform vendor's own domains (network tests, profile images, trophies, matchmaking).
   - The executable holds no complete URL to compare the result with. The final confirmation is the server log in step 3: the console's first config download shows the config host as `host=`.

**Optional confirmation with a decompiler:** load the executable (a 64-bit big-endian PowerPC ELF) in a decompiler such as Ghidra and look at the references to the two `http://%s.` strings. The online start-up function uses both: it builds both base addresses from one bucket name, then downloads the config archive from the storage-provider one. Keep the decompiler project and any exports outside this repository.

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
2. Allow inbound TCP 80 and UDP 3232–3233 on the server machine, from the local network only. Don't forward these ports on the router: everything the server serves is derived from the user's game files.
3. Start the game's online mode. The server log should show, in order:
   - `http method=GET host=<config host> path=/net-patch9.bin.psarc.crypt status=200` (twice, when the config changed since the console last downloaded it). The `host=` value confirms the config host from step 2;
   - `auth: ticket received` and `auth: echoed`;
   - `GET /build/main/text1/<n>.networking.crypt 200`;
   - `GET /userdata/... 404` (normal);
   - `presence ... heartbeat`.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| No request reaches the server | DNS still resolves the config host elsewhere (check against the console's DNS server), a proxy intercepts it, or the firewall blocks port 80 |
| Config downloaded but no auth line | `public_host` isn't reachable from the console, or UDP 3233 is blocked |
| `auth: datagram is not a login ticket with a valid online id` from the console's address | The server only answers well-formed login tickets, and this one failed the check. Report it as a bug with the `len=` value from that line |
| Console stuck on "connecting" after auth, no further requests | `script_archive` missing, or the console is using an old cached config; the server must send `Last-Modified` (it does by default) |
| "Error syncing text" | The text for the console's language index wasn't served: check `text_archive` and the `networking texts ready` log line |
| Only one `GET /net-patch9...` per start, and changes have no effect | The console's cached copy is considered current. Any change to the served config changes its date and forces a download. |
| "UNKNOWN STRING" in place of some online menu labels | The served networking text is the shipped version, which lacks strings the online config uses. See below. |

### "UNKNOWN STRING" in the online menus

The setup is working: login, presence and text sync all succeed, and gameplay is unaffected. By default the server sends each language's networking text from `text_archive`, which is the version that shipped with the game. The live service later sent a newer version of that table for some languages, with many more strings, and the online config refers to some of them. Each string missing from the shipped table shows as "UNKNOWN STRING".

**Fix:** serve a newer table for the console's language with `text_overrides` in `uc2-server.toml` (see `uc2-server.example.toml`). It maps a language index to a ready file:
```
text_overrides = { "<n>" = "<path to file>" }
```
- `<n>` is the number in the console's `GET /build/main/text1/<n>.networking.crypt` request in the server log.
- Several indexes for the same language (e.g. two English variants) can point to the same file.
- Indexes without an override still get the shipped text.

The user must provide that file; it is their responsibility. Don't explain where to get it, don't extract it from packet captures yourself, and never download it. Keep it outside the repository (absolute path) or in the git-ignored `game/` folder.

| How to recognise it | |
|---|---|
| What it is | The live form of one language's networking text: a single file, not an archive (`list-archive` doesn't apply). Named like the request path, `<n>.networking.crypt`. |
| Form | Already **encrypted**: the server sends it unchanged. Looks random, with a 24-byte header starting with the big-endian plaintext length, like every other `.crypt` body the config host serves. |
| Size | A little larger than the shipped `<n>.networking` entries in `text_archive` (those are about 12–24 KB), because it holds more strings. |

**Check:**
1. The startup log shows `networking text override index=<n> source=...` for each mapped index, and `networking texts ready ... overrides=<count>`. Those indexes no longer appear in `from_game`.
2. Re-enter online mode. The log shows `GET /build/main/text1/<n>.networking.crypt 200`, and the labels now display. If the console reports "Error syncing text" instead, the file probably isn't a valid encrypted text table for this build: remove the override and ask the user for the right file.
