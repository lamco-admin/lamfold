//! EROFS reader, clean-roomed from the public on-disk format (kernel
//! `erofs_fs.h` is SPDX MIT).
//!
//! Layout: a 128-byte superblock at byte 1024; inodes addressed by `nid`
//! (`inode = (meta_blkaddr << blkszbits) + nid*32`), in a 32-byte compact or
//! 64-byte extended form; file/dir data in `FLAT_PLAIN` (contiguous at a block
//! address) or `FLAT_INLINE` (whole blocks at a block address, the final partial
//! block packed in-line right after the inode). Directory data is an array of
//! 12-byte dirents followed by their names.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata,
    NodeId, Result, SubstrateCtx,
};

const SUPER_OFFSET: u64 = 1024;
const MAGIC: u32 = 0xE0F5_E1E2;

// datalayout (bits 1..=3 of i_format)
const FLAT_PLAIN: u8 = 0;
const FLAT_INLINE: u8 = 2;

struct ErofsInode {
    kind: FileKind,
    size: u64,
    mode: u16,
    layout: u8,
    raw_blkaddr: u32,
    inline_off: u64,
}

/// A mounted EROFS volume.
pub struct Erofs<S: BlockSource> {
    src: S,
    block_size: u64,
    meta_off: u64,
    root_nid: u64,
    /// inode cache keyed by nid (parse once).
    inodes: BTreeMap<u64, ErofsInode>,
}

impl<S: BlockSource> Erofs<S> {
    fn parse_inode(&mut self, nid: u64) -> Result<()> {
        if self.inodes.contains_key(&nid) {
            return Ok(());
        }
        let off = self.meta_off + nid * 32;
        let mut hdr = [0u8; 64];
        // compact inodes are 32 B; read 32 first, then the rest if extended.
        self.src.read_at(off, &mut hdr[..32])?;
        let format = le_u16(&hdr, 0)?;
        let extended = format & 1 != 0;
        let inode_size = if extended { 64 } else { 32 };
        if extended {
            self.src.read_at(off + 32, &mut hdr[32..64])?;
        }
        let xattr_icount = le_u16(&hdr, 2)?;
        let xattr_size = if xattr_icount == 0 {
            0u64
        } else {
            12 + (u64::from(xattr_icount) - 1) * 4
        };
        let mode = le_u16(&hdr, 4)?;
        let size = if extended {
            le_u64(&hdr, 8)?
        } else {
            u64::from(le_u32(&hdr, 8)?)
        };
        let raw_blkaddr = le_u32(&hdr, 16)?;
        let layout = ((format >> 1) & 7) as u8;
        let kind = match mode & 0xF000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xA000 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        self.inodes.insert(
            nid,
            ErofsInode {
                kind,
                size,
                mode,
                layout,
                raw_blkaddr,
                inline_off: off + inode_size + xattr_size,
            },
        );
        Ok(())
    }

    fn inode(&self, nid: u64) -> Result<&ErofsInode> {
        self.inodes.get(&nid).ok_or(FoldError::NotFound)
    }

    /// Read `buf.len()` bytes of an inode's data starting at logical `off`,
    /// resolving the FLAT_PLAIN / FLAT_INLINE split. Returns bytes read (clamped
    /// to the inode size). Does *not* verify — callers that surface file data
    /// (`read_at`) layer verification on top.
    fn read_inode_data(&mut self, nid: u64, off: u64, buf: &mut [u8]) -> Result<usize> {
        let inode = self.inode(nid)?;
        if off >= inode.size {
            return Ok(0);
        }
        let end = core::cmp::min(off + buf.len() as u64, inode.size);
        let total = (end - off) as usize;
        let bs = self.block_size;
        match inode.layout {
            FLAT_PLAIN => {
                let base = u64::from(inode.raw_blkaddr) * bs;
                self.src.read_at(base + off, &mut buf[..total])?;
                Ok(total)
            }
            FLAT_INLINE => {
                let full_bytes = (inode.size / bs) * bs;
                let base = u64::from(inode.raw_blkaddr) * bs;
                let inline_off = inode.inline_off;
                let mut p = off;
                while p < end {
                    let (disk, seg_end) = if p < full_bytes {
                        (base + p, core::cmp::min(end, full_bytes))
                    } else {
                        (inline_off + (p - full_bytes), end)
                    };
                    let n = (seg_end - p) as usize;
                    let bo = (p - off) as usize;
                    self.src.read_at(disk, &mut buf[bo..bo + n])?;
                    p = seg_end;
                }
                Ok(total)
            }
            _ => Err(FoldError::Unsupported(
                "erofs: compressed/chunk datalayout (this build reads uncompressed only)",
            )),
        }
    }

    /// Parse the dirents in one directory data block.
    fn parse_dir_block(block: &[u8], out: &mut Vec<(String, u64, u8)>) -> Result<()> {
        if block.len() < 12 {
            return Ok(());
        }
        let first_nameoff = usize::from(le_u16(block, 8)?);
        let count = first_nameoff / 12;
        for i in 0..count {
            let base = i * 12;
            let nid = le_u64(block, base)?;
            let nameoff = usize::from(le_u16(block, base + 8)?);
            let file_type = *block
                .get(base + 10)
                .ok_or(FoldError::Corrupt("erofs: dirent"))?;
            let name_end = if i + 1 < count {
                usize::from(le_u16(block, (i + 1) * 12 + 8)?)
            } else {
                block.len()
            };
            let raw = block
                .get(nameoff..name_end)
                .ok_or(FoldError::Corrupt("erofs: dirent name OOB"))?;
            // names are not NUL-terminated, but the final name in a block may be
            // zero-padded to the block boundary.
            let name = raw.split(|&b| b == 0).next().unwrap_or(raw);
            if name.is_empty() || name == b"." || name == b".." {
                continue;
            }
            out.push((String::from_utf8_lossy(name).into_owned(), nid, file_type));
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Erofs<S> {
    const TAG: &'static str = "erofs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < SUPER_OFFSET + 4 {
            return Ok(false);
        }
        let mut m = [0u8; 4];
        src.read_at(SUPER_OFFSET, &mut m)?;
        Ok(u32::from_le_bytes(m) == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        if src.len() < SUPER_OFFSET + 128 {
            return Err(FoldError::Corrupt("erofs: source shorter than superblock"));
        }
        let mut src = src;
        let mut sb = [0u8; 128];
        src.read_at(SUPER_OFFSET, &mut sb)?;
        if le_u32(&sb, 0)? != MAGIC {
            return Err(FoldError::Corrupt("erofs: bad magic"));
        }
        let blkszbits = sb[12];
        if !(9..=16).contains(&blkszbits) {
            return Err(FoldError::Corrupt("erofs: implausible blkszbits"));
        }
        let block_size = 1u64 << blkszbits;
        let root_nid = u64::from(le_u16(&sb, 14)?);
        let meta_off = u64::from(le_u32(&sb, 40)?) * block_size;
        let mut me = Erofs {
            src,
            block_size,
            meta_off,
            root_nid,
            inodes: BTreeMap::new(),
        };
        me.parse_inode(root_nid)?;
        Ok(me)
    }

    fn root(&self) -> NodeId {
        self.root_nid
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
        self.parse_inode(dir)?;
        let inode = self.inode(dir)?;
        if inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let size = checked_full_read_len(inode.size)?;
        let mut data = vec![0u8; size];
        self.read_inode_data(dir, 0, &mut data)?;

        let bs = self.block_size as usize;
        let mut raw = Vec::new();
        let mut start = 0;
        while start < data.len() {
            let block_end = core::cmp::min(start + bs, data.len());
            Self::parse_dir_block(&data[start..block_end], &mut raw)?;
            start = block_end;
        }

        let mut out = Vec::with_capacity(raw.len());
        for (name, nid, file_type) in raw {
            out.push(DirEntry {
                name,
                node: nid,
                kind: ft_kind(file_type),
            });
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        self.parse_inode(node)?;
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
            mode: u32::from(inode.mode) & 0o7777,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        self.parse_inode(node)?;
        let inode = self.inode(node)?;
        if inode.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        let size = inode.size;
        if off >= size {
            return Ok(0);
        }
        let bs = self.block_size;
        let end = core::cmp::min(off + buf.len() as u64, size);
        let mut block = vec![0u8; bs as usize];
        let mut produced = 0;
        let mut block_start = (off / bs) * bs;
        while block_start < end {
            let block_len = core::cmp::min(bs, size - block_start) as usize;
            // Materialise the whole block so it can be verified as a unit, then
            // hand it to the shepherd before any byte is surfaced.
            self.read_inode_data(node, block_start, &mut block[..block_len])?;
            cx.verifier
                .verify_block(node, block_start, &block[..block_len])?;

            let copy_start = core::cmp::max(off, block_start);
            let copy_end = core::cmp::min(end, block_start + block_len as u64);
            if copy_end > copy_start {
                let src_lo = (copy_start - block_start) as usize;
                let dst_lo = (copy_start - off) as usize;
                let cnt = (copy_end - copy_start) as usize;
                buf[dst_lo..dst_lo + cnt].copy_from_slice(&block[src_lo..src_lo + cnt]);
                produced += cnt;
            }
            block_start += bs;
        }
        Ok(produced)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        self.parse_inode(node)?;
        let inode = self.inode(node)?;
        if inode.kind != FileKind::Symlink {
            return Ok(None);
        }
        let len = checked_full_read_len(inode.size)?;
        let mut target = vec![0u8; len];
        self.read_inode_data(node, 0, &mut target)?;
        Ok(Some(target))
    }
}

fn ft_kind(file_type: u8) -> FileKind {
    match file_type {
        1 => FileKind::Regular,
        2 => FileKind::Directory,
        7 => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

fn le_u16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(FoldError::Corrupt("erofs: truncated u16"))
}
fn le_u32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(FoldError::Corrupt("erofs: truncated u32"))
}
fn le_u64(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(FoldError::Corrupt("erofs: truncated u64"))
}
