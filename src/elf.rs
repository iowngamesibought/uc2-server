//! Minimal big-endian ELF64 reader: maps virtual addresses to file bytes so the cipher keys can be
//! read from the user's executable at runtime instead of being stored anywhere.

use anyhow::{Context, Result, bail};

pub struct Elf {
    data: Vec<u8>,
    loads: Vec<(u64, u64, u64)>, // (vaddr, file offset, file size)
}

impl Elf {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if data.len() < 0x40 || &data[..4] != b"\x7fELF" || data[4] != 2 || data[5] != 2 {
            bail!("{} is not a big-endian ELF64: expected the decrypted main executable (an encrypted one is not an ELF); see AGENTS.md", path.display());
        }
        let be64 = |o: usize| u64::from_be_bytes(data[o..o + 8].try_into().unwrap());
        let be16 = |o: usize| u16::from_be_bytes(data[o..o + 2].try_into().unwrap());
        let (phoff, phnum) = (be64(0x20) as usize, be16(0x38) as usize);
        let mut loads = Vec::new();
        for i in 0..phnum {
            let ph = phoff + i * 0x38;
            let p_type = u32::from_be_bytes(data[ph..ph + 4].try_into().unwrap());
            let (off, vaddr, filesz) = (be64(ph + 8), be64(ph + 0x10), be64(ph + 0x20));
            if p_type == 1 && filesz > 0 {
                loads.push((vaddr, off, filesz));
            }
        }
        Ok(Self { data, loads })
    }

    pub fn read(&self, addr: u64, len: usize) -> Result<&[u8]> {
        for &(vaddr, off, size) in &self.loads {
            if addr >= vaddr && addr + len as u64 <= vaddr + size {
                let start = (off + addr - vaddr) as usize;
                return Ok(&self.data[start..start + len]);
            }
        }
        bail!("address {addr:#x}+{len:#x} is not file-backed")
    }

    pub fn cstr(&self, addr: u64, max: usize) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        while out.len() < max {
            let b = self.read(addr + out.len() as u64, 1)?[0];
            if b == 0 {
                return Ok(out);
            }
            out.push(b);
        }
        bail!("unterminated string at {addr:#x}")
    }
}
