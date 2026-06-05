//! cramfs reader, clean-roomed from the public format (`cramfs_fs.h`).
//!
//! Layout: a 64-byte superblock whose tail is the 12-byte root inode; inodes are
//! bitfield-packed (`mode`/`uid` in word 0, `size`/`gid` in word 1, `namelen`
//! and `offset` — both in 4-byte units — in word 2); a name follows each inode.
//! A directory's data is a run of `inode + name` records; a file's data is a
//! per-page block index (one u32 end-offset per 4 KiB page) followed by the
//! zlib-compressed pages.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, decode, BlockSource, Codec, DirEntry, FileKind, FoldError, FoldFrontend,
    Metadata, NodeId, Result, SubstrateCtx,
};

const MAGIC: u32 = 0x28CD_3D45;
const ROOT_INODE_OFF: u64 = 64;
const PAGE: u64 = 4096;
const FLAG_EXT_BLOCK_POINTERS: u32 = 0x0000_0800;

struct CrInode {
    mode: u16,
    size: u64,
    namelen: usize,
    data_off: u64,
}

/// A mounted cramfs volume.
pub struct Cramfs<S: BlockSource> {
    src: S,
}

impl<S: BlockSource> Cramfs<S> {
    fn parse_inode(&mut self, off: u64) -> Result<CrInode> {
        let mut b = [0u8; 12];
        self.src.read_at(off, &mut b)?;
        let w0 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let w1 = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        let w2 = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        Ok(CrInode {
            mode: (w0 & 0xFFFF) as u16,
            size: u64::from(w1 & 0x00FF_FFFF),
            namelen: (w2 & 0x3F) as usize * 4,
            data_off: u64::from(w2 >> 6) * 4,
        })
    }

    fn kind(mode: u16) -> FileKind {
        match mode & 0xF000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xA000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }

    /// Inflate a whole file/symlink (per-page zlib) into a fresh buffer.
    fn read_whole(&mut self, inode: &CrInode) -> Result<Vec<u8>> {
        let total = checked_full_read_len(inode.size)?;
        if total == 0 {
            return Ok(Vec::new());
        }
        let nblocks = inode.size.div_ceil(PAGE) as usize;
        // block index: one u32 end-offset per page
        let mut idx = vec![0u8; nblocks * 4];
        self.src.read_at(inode.data_off, &mut idx)?;
        let ptr = |i: usize| {
            u32::from_le_bytes([idx[i * 4], idx[i * 4 + 1], idx[i * 4 + 2], idx[i * 4 + 3]]) as u64
        };

        let mut out = Vec::with_capacity(total);
        let mut start = inode.data_off + nblocks as u64 * 4;
        for i in 0..nblocks {
            let end = ptr(i);
            let want = core::cmp::min(PAGE as usize, total - out.len());
            if end <= start {
                // a hole: the page is all zeros
                out.resize(out.len() + want, 0);
            } else {
                let mut comp = vec![0u8; (end - start) as usize];
                self.src.read_at(start, &mut comp)?;
                out.extend_from_slice(&decode(Codec::Zlib, &comp, want)?);
            }
            start = end;
        }
        out.truncate(total);
        Ok(out)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Cramfs<S> {
    const TAG: &'static str = "cramfs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < ROOT_INODE_OFF + 12 {
            return Ok(false);
        }
        let mut m = [0u8; 4];
        src.read_at(0, &mut m)?;
        Ok(u32::from_le_bytes(m) == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut src = src;
        if src.len() < ROOT_INODE_OFF + 12 {
            return Err(FoldError::Corrupt("cramfs: shorter than superblock"));
        }
        let mut head = [0u8; 12];
        src.read_at(0, &mut head)?;
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        if magic != MAGIC {
            return Err(if magic.swap_bytes() == MAGIC {
                FoldError::Unsupported("cramfs: big-endian image")
            } else {
                FoldError::Corrupt("cramfs: bad magic")
            });
        }
        let flags = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        if flags & FLAG_EXT_BLOCK_POINTERS != 0 {
            return Err(FoldError::Unsupported(
                "cramfs: EXT_BLOCK_POINTERS extension",
            ));
        }
        Ok(Cramfs { src })
    }

    fn root(&self) -> NodeId {
        ROOT_INODE_OFF
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
        let inode = self.parse_inode(dir)?;
        if Self::kind(inode.mode) != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let end = inode.data_off + inode.size;
        let mut out = Vec::new();
        let mut p = inode.data_off;
        while p < end {
            let child = self.parse_inode(p)?;
            let mut name = vec![0u8; child.namelen];
            self.src.read_at(p + 12, &mut name)?;
            let name = name.split(|&b| b == 0).next().unwrap_or(&name);
            out.push(DirEntry {
                name: String::from_utf8_lossy(name).into_owned(),
                node: p,
                kind: Self::kind(child.mode),
            });
            p += 12 + child.namelen as u64;
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.parse_inode(node)?;
        Ok(Metadata {
            kind: Self::kind(inode.mode),
            size: inode.size,
            mode: u32::from(inode.mode) & 0o7777,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        _cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.parse_inode(node)?;
        if Self::kind(inode.mode) == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        if off >= inode.size {
            return Ok(0);
        }
        let data = self.read_whole(&inode)?;
        let start = off as usize;
        let n = core::cmp::min(buf.len(), data.len().saturating_sub(start));
        buf[..n].copy_from_slice(&data[start..start + n]);
        Ok(n)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.parse_inode(node)?;
        if Self::kind(inode.mode) != FileKind::Symlink {
            return Ok(None);
        }
        Ok(Some(self.read_whole(&inode)?))
    }
}
