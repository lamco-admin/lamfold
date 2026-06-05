//! Linux romfs reader, clean-roomed from the public `-rom1fs-` format.
//!
//! Layout: an 8-byte magic, big-endian 32-bit size + checksum, a NUL-terminated
//! volume name padded to 16 bytes; then a chain of file headers, each 16-byte
//! aligned: `next` (low 4 bits = type/exec, high bits = next header offset),
//! `info` (first child for a directory), `size`, checksum, a NUL-terminated name
//! padded to 16, then the file data. Everything is big-endian and uncompressed.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata,
    NodeId, Result, SubstrateCtx,
};

const MAGIC: &[u8; 8] = b"-rom1fs-";
const MAX_NAME: usize = 256;

// file types (low 3 bits of `next`); hard link (0) and devices/fifo/socket
// (4..=7) fall through to FileKind::Other.
const FT_DIRECTORY: u32 = 1;
const FT_REGULAR: u32 = 2;
const FT_SYMLINK: u32 = 3;

struct Header {
    next_off: u64,
    ftype: u32,
    info: u64,
    size: u64,
    name: String,
    data_off: u64,
}

/// A mounted romfs volume.
pub struct Romfs<S: BlockSource> {
    src: S,
    first_hdr: u64,
}

impl<S: BlockSource> Romfs<S> {
    fn read_header(&mut self, off: u64) -> Result<Header> {
        let avail = core::cmp::min((16 + MAX_NAME) as u64, self.src.len().saturating_sub(off));
        if avail < 16 {
            return Err(FoldError::Corrupt("romfs: truncated header"));
        }
        let mut win = vec![0u8; avail as usize];
        self.src.read_at(off, &mut win)?;
        let next_raw = be_u32(&win, 0)?;
        let info = u64::from(be_u32(&win, 4)?);
        let size = u64::from(be_u32(&win, 8)?);
        let name_bytes = &win[16..];
        let nul = name_bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or(FoldError::Corrupt("romfs: unterminated name"))?;
        let name = String::from_utf8_lossy(&name_bytes[..nul]).into_owned();
        Ok(Header {
            next_off: u64::from(next_raw & 0xFFFF_FFF0),
            ftype: next_raw & 7,
            info,
            size,
            data_off: off + 16 + align16(nul + 1) as u64,
            name,
        })
    }

    fn kind(ftype: u32) -> FileKind {
        match ftype {
            FT_DIRECTORY => FileKind::Directory,
            FT_REGULAR => FileKind::Regular,
            FT_SYMLINK => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
}

impl<S: BlockSource> FoldFrontend<S> for Romfs<S> {
    const TAG: &'static str = "romfs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < 16 {
            return Ok(false);
        }
        let mut m = [0u8; 8];
        src.read_at(0, &mut m)?;
        Ok(&m == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut src = src;
        if src.len() < 16 {
            return Err(FoldError::Corrupt("romfs: shorter than header"));
        }
        let mut head = [0u8; 16 + MAX_NAME];
        let avail = core::cmp::min((16 + MAX_NAME) as u64, src.len()) as usize;
        src.read_at(0, &mut head[..avail])?;
        if &head[..8] != MAGIC {
            return Err(FoldError::Corrupt("romfs: bad magic"));
        }
        // volume name: NUL-terminated from byte 16, padded to 16.
        let nul = head[16..avail]
            .iter()
            .position(|&b| b == 0)
            .ok_or(FoldError::Corrupt("romfs: unterminated volume name"))?;
        let first_hdr = 16 + align16(nul + 1) as u64;
        Ok(Romfs { src, first_hdr })
    }

    fn root(&self) -> NodeId {
        // The first file header is the root directory's first entry ("."), and
        // its own `info` points back here — so it serves as the root node.
        self.first_hdr
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .read_dir(dir, cx)?
            .into_iter()
            .find(|e| e.name == name)
            .map(|e| e.node))
    }

    fn read_dir(&mut self, dir: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let head = self.read_header(dir)?;
        if head.ftype != FT_DIRECTORY {
            return Err(FoldError::NotDirectory);
        }
        let mut out = Vec::new();
        let mut o = head.info; // first child header
                               // bound the walk so a cyclic `next` can't spin forever
        for _ in 0..1_000_000 {
            if o == 0 {
                break;
            }
            let h = self.read_header(o)?;
            if h.name != "." && h.name != ".." {
                out.push(DirEntry {
                    name: h.name,
                    node: o,
                    kind: Self::kind(h.ftype),
                });
            }
            o = h.next_off;
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let h = self.read_header(node)?;
        Ok(Metadata {
            kind: Self::kind(h.ftype),
            size: h.size,
            mode: 0,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        _cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let h = self.read_header(node)?;
        if h.ftype == FT_DIRECTORY {
            return Err(FoldError::IsDirectory);
        }
        if off >= h.size {
            return Ok(0);
        }
        let n = core::cmp::min(buf.len() as u64, h.size - off) as usize;
        self.src.read_at(h.data_off + off, &mut buf[..n])?;
        Ok(n)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let h = self.read_header(node)?;
        if h.ftype != FT_SYMLINK {
            return Ok(None);
        }
        let len = checked_full_read_len(h.size)?;
        let mut target = vec![0u8; len];
        self.src.read_at(h.data_off, &mut target)?;
        Ok(Some(target))
    }
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn be_u32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or(FoldError::Corrupt("romfs: truncated u32"))
}
