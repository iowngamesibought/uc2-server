//! Builds the client's online config: a DC00 script module (net.bin), packed into a game archive (format
//! version 1.3) and encrypted as a stream; the client requests it as `<name>.psarc.crypt`.
//!
//! DC00 layout: "DC00" | u32 1 | u32 reloc table offset | u32 0 | u32 root pointer, then data. The
//! root table is { u32 1, u32 count, { u32 sid, u32 value pointer }[count] } sorted by sid. The
//! reloc table is { u32 count, u32 offset[count] }: each listed word is a module-relative pointer.

use std::collections::HashMap;
use std::io::Write;

use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use serde::Deserialize;

use crate::cipher::BlobCipher;

pub const SID_SERVER_CONFIG_A: u32 = 0x9BD0_38AC; // used for login ticket issuer ids <= 7
pub const SID_SERVER_CONFIG_B: u32 = 0x1D38_F2F9; // issuer ids > 7
const TYPE_STRING_ARRAY: u32 = 0x0B39_52E7;
const DC_HDR_04: u32 = 1;
const DC_HDR_0C: u32 = 0;
const DC_ROOT_00: u32 = 1;
const MAX_MODULE: usize = 0x40000; // the client's load buffer

/// Names the client downloads as `<name>.psarc.crypt`: net.bin for earlier builds, net-patch9.bin
/// for the GOTY 01.10 build.
pub const REQUESTED_NAMES: [&str; 2] = ["net.bin", "net-patch9.bin"];

/// One server-config object (sid 0x9bd038ac / 0x1d38f2f9), 0x44 bytes in the module.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerConfig {
    pub host_a: String,
    pub port_a: u32,
    pub host_b: String,
    pub port_b: u32,
    pub auth_host: String,
    pub auth_port: u32,
    pub auth_host2: String,
    #[serde(default)]
    pub stats_prefix: Option<String>,
    pub stats_queue: String,
    pub stats_queues: Vec<String>,
    pub tus_mirror_queue: String,
    pub nat_queue: String,
    #[serde(default = "d20")]
    pub session_minimum: u32,
    #[serde(default = "d12")]
    pub net_game_param44: u32,
    #[serde(default = "d32")]
    pub param34: u32,
    #[serde(default = "d64")]
    pub param38: u32,
    #[serde(default)]
    pub tag40: u32,
}

fn d20() -> u32 {
    20
}
fn d12() -> u32 {
    12
}
fn d32() -> u32 {
    32
}
fn d64() -> u32 {
    64
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetBinConfig {
    pub config_a: ServerConfig,
    pub config_b: ServerConfig,
}

struct Builder {
    buf: Vec<u8>,
    relocs: Vec<usize>,
}

impl Builder {
    fn new() -> Self {
        Self {
            buf: vec![0; 0x10],
            relocs: Vec::new(),
        }
    }
    fn here(&self) -> usize {
        self.buf.len()
    }
    fn align(&mut self, n: usize) -> usize {
        while self.buf.len() % n != 0 {
            self.buf.push(0)
        }
        self.buf.len()
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes())
    }
    fn ptr(&mut self, target: Option<usize>) {
        match target {
            None => self.u32(0),
            Some(t) => {
                self.relocs.push(self.buf.len());
                self.u32(t as u32);
            }
        }
    }
    fn cstring(&mut self, s: &str) -> usize {
        let off = self.buf.len();
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
        off
    }
}

fn server_config(b: &mut Builder, c: &ServerConfig) -> usize {
    // Unique strings in first-use order (pinned by the module_bytes_are_stable test).
    let fields: [Option<&str>; 8] = [
        Some(&c.host_a),
        Some(&c.host_b),
        Some(&c.auth_host),
        c.stats_prefix.as_deref(),
        Some(&c.stats_queue),
        Some(&c.auth_host2),
        Some(&c.tus_mirror_queue),
        Some(&c.nat_queue),
    ];
    let mut order: Vec<&str> = Vec::new();
    for s in fields
        .iter()
        .flatten()
        .copied()
        .chain(c.stats_queues.iter().map(String::as_str))
    {
        if !order.contains(&s) {
            order.push(s);
        }
    }
    let mut at: HashMap<&str, usize> = HashMap::new();
    for s in &order {
        at.insert(s, b.cstring(s));
    }
    b.align(4);
    let items = b.here();
    for q in &c.stats_queues {
        b.ptr(Some(at[q.as_str()]))
    }
    let array = b.here();
    b.u32(c.stats_queues.len() as u32);
    b.ptr(Some(items));
    b.u32(TYPE_STRING_ARRAY);
    b.align(16);
    let obj = b.here();
    let p = |f: Option<&str>| f.map(|s| at[s]);
    b.ptr(p(fields[0]));
    b.u32(c.port_a); // +0x00 hostA, +0x04 presence port (u16 at +6)
    b.ptr(p(fields[1]));
    b.u32(c.port_b); // +0x08 hostB, +0x0c
    b.ptr(p(fields[2]));
    b.u32(c.auth_port); // +0x10 auth host (also the presence IP), +0x14 auth port
    b.ptr(p(fields[3])); // +0x18 stats upload prefix (NULL = disabled)
    b.ptr(p(fields[4])); // +0x1c SQS stats queue
    b.ptr(Some(array)); // +0x20 typed array of queue paths
    b.u32(c.session_minimum); // +0x24
    b.u32(c.net_game_param44); // +0x28
    b.ptr(p(fields[5])); // +0x2c second auth host
    b.ptr(p(fields[6])); // +0x30 TUS mirror queue
    b.u32(c.param34); // +0x34
    b.u32(c.param38); // +0x38
    b.ptr(p(fields[7])); // +0x3c NAT queue
    b.u32(c.tag40); // +0x40
    debug_assert_eq!(b.here() - obj, 0x44);
    obj
}

/// The DC00 module bytes.
pub fn build_module(cfg: &NetBinConfig) -> Result<Vec<u8>> {
    let mut b = Builder::new();
    b.u32(0); // 0x10: root pointer, patched below
    let mut syms = vec![
        (SID_SERVER_CONFIG_A, &cfg.config_a),
        (SID_SERVER_CONFIG_B, &cfg.config_b),
    ];
    syms.sort_by_key(|(sid, _)| *sid);
    let values: Vec<(u32, usize)> = syms
        .iter()
        .map(|(sid, c)| (*sid, server_config(&mut b, c)))
        .collect();
    b.align(16);
    let root = b.here();
    b.u32(DC_ROOT_00);
    b.u32(values.len() as u32);
    for (sid, obj) in &values {
        b.u32(*sid);
        b.ptr(Some(*obj));
    }
    b.buf[0x10..0x14].copy_from_slice(&(root as u32).to_be_bytes());
    b.relocs.push(0x10);
    let reloc_off = b.align(4);
    let mut relocs = b.relocs.clone();
    relocs.sort_unstable();
    b.u32(relocs.len() as u32);
    for r in relocs {
        b.u32(r as u32)
    }
    b.align(16);
    b.buf[0..4].copy_from_slice(b"DC00");
    b.buf[4..8].copy_from_slice(&DC_HDR_04.to_be_bytes());
    b.buf[8..12].copy_from_slice(&(reloc_off as u32).to_be_bytes());
    b.buf[12..16].copy_from_slice(&DC_HDR_0C.to_be_bytes());
    if b.buf.len() > MAX_MODULE {
        bail!(
            "net.bin is {} bytes; the client's load buffer is {MAX_MODULE:#x}",
            b.buf.len()
        );
    }
    Ok(b.buf)
}

/// Character pak list: {count, items, type}.
const SID_ACTOR_LIST: u32 = 0x5A47_31D2;
/// {count, items} of 8-byte pairs the GOTY 01.10 client reads during its online start. The shipped
/// net.bin does not define it, so it is added empty.
const SID_PATCH9_PAIRS: u32 = 0x9F9B_CB43;
/// {count, items, type} of pak diffs the client downloads from the content host at online start. It
/// is served empty: the GOTY 01.10 install already contains the patched paks.
const SID_PAK_DIFFS: u32 = 0x3D24_05AC;

/// Rewrites the character pak list from the shipped net.bin's 0x20-byte entries to the 0x24-byte
/// entries the GOTY 01.10 client reads: +8 name, +0xc sid, +0x1c download flag, +0x20 (new; when
/// non-zero the entry is registered with that value). The new word is 0. The download flag is
/// cleared because the GOTY 01.10 install contains every listed pak, so they load locally instead
/// of from the content host.
fn widen_actor_list(b: &mut Builder, base: &[u8], relocs: &[usize], list: usize) -> Result<usize> {
    let (count, items, ty) = (
        be32(base, list)?,
        be32(base, list + 4)?,
        be32(base, list + 8)? as u32,
    );
    let is_ptr = |o: usize| relocs.binary_search(&o).is_ok();
    if count > 1 && !is_ptr(items + 0x20 + 8) {
        if is_ptr(items + 0x24 + 8) {
            return Ok(list); // already in the 0x24 layout
        }
        bail!("actor list at {list:#x}: unrecognised entry layout");
    }
    let new_items = b.align(16);
    for i in 0..count {
        for w in 0..8 {
            let o = items + 0x20 * i + 4 * w;
            let v = be32(base, o)?;
            match w {
                7 => b.u32(0),
                _ if is_ptr(o) => b.ptr(Some(v)),
                _ => b.u32(v as u32),
            }
        }
        b.u32(0);
    }
    let obj = b.align(16);
    b.u32(count as u32);
    b.ptr(Some(new_items));
    b.u32(ty);
    Ok(obj)
}

fn be32(b: &[u8], o: usize) -> Result<usize> {
    match b.get(o..o + 4) {
        Some(w) => Ok(u32::from_be_bytes(w.try_into().unwrap()) as usize),
        None => bail!("DC module truncated at {o:#x}"),
    }
}

/// The user's own net module with our two server-config objects swapped in.
///
/// The downloaded module's symbols replace those of the client's local net.bin, which defines many
/// more than the server configs (download lists and other online settings). A module holding only
/// the server configs would drop them. The shipped net.bin is older than the GOTY 01.10 client, so
/// the character pak list is widened (widen_actor_list), the missing SID_PATCH9_PAIRS list is added
/// empty and the pak-diff list is emptied (SID_PAK_DIFFS). The base module's bytes before its reloc
/// table are kept, new objects and a new sorted root table are appended, the header's root pointer
/// moves to it, and the reloc table is rebuilt.
pub fn merge_module(base: &[u8], cfg: &NetBinConfig) -> Result<Vec<u8>> {
    if base.get(..4) != Some(b"DC00") {
        bail!("base net module is not a DC00 module");
    }
    let reloc_off = be32(base, 8)?;
    let mut relocs = (0..be32(base, reloc_off)?)
        .map(|i| be32(base, reloc_off + 4 + 4 * i))
        .collect::<Result<Vec<_>>>()?;
    let root = be32(base, 0x10)?;
    let mut symbols = (0..be32(base, root + 4)?)
        .map(|i| {
            Ok((
                be32(base, root + 8 + 8 * i)? as u32,
                be32(base, root + 12 + 8 * i)?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<u32, usize>>>()?;
    let mut b = Builder {
        buf: base[..reloc_off].to_vec(),
        relocs: Vec::new(),
    };
    b.align(16);
    for (sid, c) in [
        (SID_SERVER_CONFIG_A, &cfg.config_a),
        (SID_SERVER_CONFIG_B, &cfg.config_b),
    ] {
        if !symbols.contains_key(&sid) {
            bail!("base net module has no symbol {sid:#010x}");
        }
        let obj = server_config(&mut b, c);
        b.align(16);
        symbols.insert(sid, obj);
    }
    if let Some(&list) = symbols.get(&SID_ACTOR_LIST) {
        let obj = widen_actor_list(&mut b, base, &relocs, list)?;
        symbols.insert(SID_ACTOR_LIST, obj);
    }
    symbols.entry(SID_PATCH9_PAIRS).or_insert_with(|| {
        let obj = b.align(16);
        b.u32(0);
        b.u32(0);
        b.u32(0);
        obj
    });
    if let Some(&list) = symbols.get(&SID_PAK_DIFFS) {
        let obj = b.align(16);
        b.u32(0);
        b.u32(0);
        b.u32(be32(base, list + 8)? as u32);
        symbols.insert(SID_PAK_DIFFS, obj);
    }
    // New root table (the base one cannot grow in place); the old one stays as unused data.
    let new_root = b.align(16);
    b.u32(be32(base, root)? as u32);
    b.u32(symbols.len() as u32);
    for (&sid, &obj) in &symbols {
        b.u32(sid);
        b.ptr(Some(obj));
    }
    b.buf[0x10..0x14].copy_from_slice(&(new_root as u32).to_be_bytes());
    relocs.extend(b.relocs.iter().copied());
    relocs.sort_unstable();
    relocs.dedup();
    let new_reloc_off = b.align(4);
    b.u32(relocs.len() as u32);
    for r in relocs {
        b.u32(r as u32)
    }
    b.align(16);
    b.buf[8..12].copy_from_slice(&(new_reloc_off as u32).to_be_bytes());
    if b.buf.len() > MAX_MODULE {
        bail!(
            "merged net module is {} bytes; the client's load buffer is {MAX_MODULE:#x}",
            b.buf.len()
        );
    }
    Ok(b.buf)
}

/// The online config module's entry: `net.bin` at the root (the base game's script archive) or
/// under a folder (an update's archive, whose entries carry full paths).
pub fn net_module_entry(names: &[String]) -> Option<&str> {
    names
        .iter()
        .map(String::as_str)
        .find(|n| *n == "net.bin" || n.ends_with("/net.bin"))
}

/// One entry of a game archive (zlib, any block size), by manifest path.
pub fn read_archive_entry(blob: &[u8], name: &str) -> Result<Vec<u8>> {
    let index = archive_manifest(blob)?
        .iter()
        .position(|p| p == name)
        .with_context(|| format!("archive has no entry {name}"))?;
    archive_extract(blob, index + 1)
}

/// The entry paths of a game archive, in TOC order (entry 0, the manifest itself, excluded).
pub fn archive_manifest(blob: &[u8]) -> Result<Vec<String>> {
    let manifest = archive_extract(blob, 0)?;
    Ok(String::from_utf8_lossy(&manifest)
        .split('\n')
        .map(str::to_owned)
        .collect())
}

fn archive_extract(blob: &[u8], entry: usize) -> Result<Vec<u8>> {
    if blob.get(..4) != Some(b"PSAR") {
        bail!("not a game archive");
    }
    let (toc_len, entry_size, count, block) = (
        be32(blob, 12)?,
        be32(blob, 16)?,
        be32(blob, 20)?,
        be32(blob, 24)?,
    );
    let width = if block <= 0x10000 {
        2
    } else if block <= 0x100_0000 {
        3
    } else {
        4
    };
    let table = 32 + count * entry_size;
    let sizes: Vec<usize> = blob
        .get(table..toc_len)
        .context("archive block table truncated")?
        .chunks_exact(width)
        .map(|c| c.iter().fold(0usize, |a, &x| a << 8 | x as usize))
        .collect();
    let extract = |i: usize| -> Result<Vec<u8>> {
        let e = 32 + i * entry_size;
        let toc = blob.get(e..e + 30).context("archive TOC truncated")?;
        let mut blk = u32::from_be_bytes(toc[16..20].try_into().unwrap()) as usize;
        let size = toc[20..25].iter().fold(0usize, |a, &x| a << 8 | x as usize);
        let mut pos = toc[25..30].iter().fold(0usize, |a, &x| a << 8 | x as usize);
        // The size comes from the file: reserve at most 16 MiB up front, grow if it is real.
        let mut out = Vec::with_capacity(size.min(1 << 24));
        while out.len() < size {
            let stored = match sizes.get(blk).context("archive block index out of range")? {
                0 => block,
                &s => s,
            };
            let chunk = blob
                .get(pos..pos + stored)
                .context("archive data truncated")?;
            let want = block.min(size - out.len());
            if stored != want && chunk.first() == Some(&0x78) {
                std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(chunk), &mut out)?;
            } else {
                out.extend_from_slice(chunk);
            }
            pos += stored;
            blk += 1;
        }
        out.truncate(size);
        Ok(out)
    };
    if entry >= count {
        bail!("archive has no entry #{entry}");
    }
    extract(entry)
}

/// Game archive (format version 1.3), zlib, 64 KiB blocks, flags 0, entry 0 = manifest.
pub fn build_archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    const BLOCK: usize = 0x10000;
    let manifest = files.iter().map(|(p, _)| *p).collect::<Vec<_>>().join("\n");
    let mut entries: Vec<([u8; 16], &[u8])> = vec![([0; 16], manifest.as_bytes())];
    for (p, d) in files {
        entries.push((Md5::digest(p.as_bytes()).into(), d));
    }
    let (mut blocks, mut data, mut toc) = (Vec::<u16>::new(), Vec::<u8>::new(), Vec::new());
    for (md5, payload) in &entries {
        let first = blocks.len() as u32;
        let offset = data.len();
        for chunk in payload.chunks(BLOCK) {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
            z.write_all(chunk).expect("in-memory write");
            let packed = z.finish().expect("in-memory write");
            let stored: &[u8] = if packed.len() < chunk.len() {
                &packed
            } else {
                chunk
            };
            blocks.push(if stored.len() == BLOCK {
                0
            } else {
                stored.len() as u16
            });
            data.extend_from_slice(stored);
        }
        toc.push((*md5, first, payload.len() as u64, offset as u64));
    }
    let toc_len = 32 + 30 * toc.len() + 2 * blocks.len();
    let mut out = Vec::with_capacity(toc_len + data.len());
    out.extend_from_slice(b"PSAR");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(b"zlib");
    for v in [toc_len as u32, 30, toc.len() as u32, BLOCK as u32, 0] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    for (md5, first, size, offset) in toc {
        out.extend_from_slice(&md5);
        out.extend_from_slice(&first.to_be_bytes());
        out.extend_from_slice(&size.to_be_bytes()[3..]);
        out.extend_from_slice(&(toc_len as u64 + offset).to_be_bytes()[3..]);
    }
    for s in blocks {
        out.extend_from_slice(&s.to_be_bytes());
    }
    out.extend_from_slice(&data);
    out
}

pub struct NetBinArtifacts {
    pub module: Vec<u8>,
    pub archive: Vec<u8>,
    pub crypt: Vec<u8>,
}

/// `base`: the user's own net module (see merge_module); None builds a module with only the server
/// configs, which is enough for config parsing and login but drops the other net.bin symbols.
pub fn build_all(
    cfg: &NetBinConfig,
    base: Option<&[u8]>,
    cipher: &BlobCipher,
) -> Result<NetBinArtifacts> {
    let module = match base {
        Some(base) => merge_module(base, cfg)?,
        None => build_module(cfg)?,
    };
    let archive = build_archive(&[("net.bin", &module)]);
    let crypt = cipher.encrypt_stream(&archive);
    Ok(NetBinArtifacts {
        module,
        archive,
        crypt,
    })
}

#[cfg(test)]
mod test;
