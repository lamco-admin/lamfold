//! SquashFS 4.0 reader, clean-roomed from the public on-disk format docs.
//!
//! Layout: a 96-byte superblock; inodes and directory listings packed into
//! compressed 8 KiB "metadata blocks" (a 2-byte header per block, top bit =
//! uncompressed); file data in `block_size` data blocks plus a shared fragment
//! for tails. All decompression goes through the shared substrate codec.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, decode, BlockSource, Codec, DirEntry, FileKind, FoldError, FoldFrontend,
    Metadata, NodeId, Result, SubstrateCtx,
};

const MAGIC: u32 = 0x7371_7368; // "hsqs"
const META_MAX: usize = 8192; // max uncompressed metadata block
const NO_FRAGMENT: u32 = 0xFFFF_FFFF;
const SIZE_UNCOMPRESSED: u32 = 1 << 24; // block-size flag: stored uncompressed
const SIZE_MASK: u32 = 0x00FF_FFFF;

#[derive(Clone)]
enum Body {
    Dir {
        abs_block: u64,
        offset: u16,
        size: u32,
    },
    File {
        blocks_start: u64,
        block_sizes: Vec<u32>,
        frag_idx: u32,
        frag_off: u32,
    },
    Symlink(Vec<u8>),
    Other,
}

#[derive(Clone)]
struct SquashInode {
    kind: FileKind,
    size: u64,
    body: Body,
}

/// A mounted SquashFS volume.
pub struct SquashFs<S: BlockSource> {
    src: S,
    codec: Codec,
    block_size: u32,
    inode_table_start: u64,
    directory_table_start: u64,
    fragment_table_start: u64,
    nodes: Vec<SquashInode>,
    by_ref: BTreeMap<u64, NodeId>,
}

impl<S: BlockSource> SquashFs<S> {
    /// Decompress one metadata block at absolute offset `at`; returns its bytes
    /// and the offset of the following block.
    fn read_meta_block(&mut self, at: u64) -> Result<(Vec<u8>, u64)> {
        let mut hdr = [0u8; 2];
        self.src.read_at(at, &mut hdr)?;
        let h = u16::from_le_bytes(hdr);
        let uncompressed = h & 0x8000 != 0;
        let len = usize::from(h & 0x7FFF);
        let mut raw = vec![0u8; len];
        self.src.read_at(at + 2, &mut raw)?;
        let data = if uncompressed {
            raw
        } else {
            decode(self.codec, &raw, META_MAX)?
        };
        Ok((data, at + 2 + len as u64))
    }

    /// Read `length` bytes starting at `offset` within the metadata block at
    /// absolute offset `abs_block`, spanning into following blocks as needed.
    fn read_meta_span(&mut self, abs_block: u64, offset: usize, length: usize) -> Result<Vec<u8>> {
        let _ = checked_full_read_len(length as u64)?;
        let mut out = Vec::with_capacity(length);
        let (mut block, mut next) = self.read_meta_block(abs_block)?;
        let mut pos = offset;
        while out.len() < length {
            if pos >= block.len() {
                let (b, n) = self.read_meta_block(next)?;
                block = b;
                next = n;
                pos = 0;
                if block.is_empty() {
                    break;
                }
            }
            let take = core::cmp::min(length - out.len(), block.len() - pos);
            out.extend_from_slice(&block[pos..pos + take]);
            pos += take;
        }
        Ok(out)
    }

    fn block_count(&self, size: u64, frag_idx: u32) -> usize {
        let bs = u64::from(self.block_size);
        if frag_idx != NO_FRAGMENT {
            (size / bs) as usize
        } else {
            size.div_ceil(bs) as usize
        }
    }

    /// Parse the inode at the given inode reference `(block << 16) | offset`.
    fn parse_inode(&mut self, inode_ref: u64) -> Result<SquashInode> {
        let abs = self.inode_table_start + (inode_ref >> 16);
        let off = (inode_ref & 0xFFFF) as usize;
        let itype = le_u16(&self.read_meta_span(abs, off, 16)?, 0)?;
        match itype {
            1 => {
                let b = self.read_meta_span(abs, off, 32)?;
                Ok(SquashInode {
                    kind: FileKind::Directory,
                    size: u64::from(le_u16(&b, 24)?),
                    body: Body::Dir {
                        abs_block: self.directory_table_start + u64::from(le_u32(&b, 16)?),
                        offset: le_u16(&b, 26)?,
                        size: u32::from(le_u16(&b, 24)?),
                    },
                })
            }
            8 => {
                let b = self.read_meta_span(abs, off, 40)?;
                let size = le_u32(&b, 20)?;
                Ok(SquashInode {
                    kind: FileKind::Directory,
                    size: u64::from(size),
                    body: Body::Dir {
                        abs_block: self.directory_table_start + u64::from(le_u32(&b, 24)?),
                        offset: le_u16(&b, 34)?,
                        size,
                    },
                })
            }
            2 => {
                let h = self.read_meta_span(abs, off, 32)?;
                let blocks_start = u64::from(le_u32(&h, 16)?);
                let frag_idx = le_u32(&h, 20)?;
                let frag_off = le_u32(&h, 24)?;
                let size = u64::from(le_u32(&h, 28)?);
                let n = self.block_count(size, frag_idx);
                let full = self.read_meta_span(abs, off, 32 + n * 4)?;
                let block_sizes = (0..n)
                    .map(|i| le_u32(&full, 32 + i * 4))
                    .collect::<Result<_>>()?;
                Ok(SquashInode {
                    kind: FileKind::Regular,
                    size,
                    body: Body::File {
                        blocks_start,
                        block_sizes,
                        frag_idx,
                        frag_off,
                    },
                })
            }
            9 => {
                let h = self.read_meta_span(abs, off, 56)?;
                let blocks_start = le_u64(&h, 16)?;
                let size = le_u64(&h, 24)?;
                let frag_idx = le_u32(&h, 44)?;
                let frag_off = le_u32(&h, 48)?;
                let n = self.block_count(size, frag_idx);
                let full = self.read_meta_span(abs, off, 56 + n * 4)?;
                let block_sizes = (0..n)
                    .map(|i| le_u32(&full, 56 + i * 4))
                    .collect::<Result<_>>()?;
                Ok(SquashInode {
                    kind: FileKind::Regular,
                    size,
                    body: Body::File {
                        blocks_start,
                        block_sizes,
                        frag_idx,
                        frag_off,
                    },
                })
            }
            3 | 10 => {
                let h = self.read_meta_span(abs, off, 24)?;
                let target_size = checked_full_read_len(u64::from(le_u32(&h, 20)?))?;
                let full = self.read_meta_span(abs, off, 24 + target_size)?;
                let target = full
                    .get(24..24 + target_size)
                    .ok_or(FoldError::Corrupt("squashfs: symlink target OOB"))?
                    .to_vec();
                Ok(SquashInode {
                    kind: FileKind::Symlink,
                    size: target_size as u64,
                    body: Body::Symlink(target),
                })
            }
            _ => Ok(SquashInode {
                kind: FileKind::Other,
                size: 0,
                body: Body::Other,
            }),
        }
    }

    fn intern(&mut self, inode_ref: u64, inode: SquashInode) -> NodeId {
        if let Some(&id) = self.by_ref.get(&inode_ref) {
            return id;
        }
        let id = self.nodes.len() as NodeId;
        self.nodes.push(inode);
        self.by_ref.insert(inode_ref, id);
        id
    }

    fn inode(&self, node: NodeId) -> Result<SquashInode> {
        self.nodes
            .get(node as usize)
            .cloned()
            .ok_or(FoldError::NotFound)
    }

    /// Parse a directory listing into (name, child inode reference) pairs.
    fn dir_entries(
        &mut self,
        abs_block: u64,
        offset: u16,
        size: u32,
    ) -> Result<Vec<(String, u64)>> {
        // `size` includes a 3-byte bias; a listing of <= 3 is empty.
        let listing = self.read_meta_span(
            abs_block,
            usize::from(offset),
            (size as usize).saturating_sub(3),
        )?;
        let mut out = Vec::new();
        let mut p = 0;
        while p + 12 <= listing.len() {
            let count = le_u32(&listing, p)? as usize; // entries - 1
            let start = u64::from(le_u32(&listing, p + 4)?); // inode metadata block
            p += 12;
            for _ in 0..count.saturating_add(1) {
                if p + 8 > listing.len() {
                    break;
                }
                let eoff = le_u16(&listing, p)?;
                let nlen = usize::from(le_u16(&listing, p + 6)?) + 1;
                let name = listing
                    .get(p + 8..p + 8 + nlen)
                    .ok_or(FoldError::Corrupt("squashfs: dir entry name OOB"))?;
                out.push((
                    String::from_utf8_lossy(name).into_owned(),
                    (start << 16) | u64::from(eoff),
                ));
                p += 8 + nlen;
            }
        }
        Ok(out)
    }

    /// Read a fragment block (the shared tail block) and return its decompressed
    /// bytes.
    fn read_fragment(&mut self, idx: u32) -> Result<Vec<u8>> {
        // The fragment table is an array of u64 pointers to metadata blocks of
        // 16-byte fragment entries (512 per 8 KiB block).
        let meta_blk = u64::from(idx / 512);
        let in_block = (idx % 512) as usize;
        let mut ptr = [0u8; 8];
        self.src
            .read_at(self.fragment_table_start + meta_blk * 8, &mut ptr)?;
        let entry_block = u64::from_le_bytes(ptr);
        let fe = self.read_meta_span(entry_block, in_block * 16, 16)?;
        let start = le_u64(&fe, 0)?;
        let raw_size = le_u32(&fe, 8)?;
        self.read_data_block(start, raw_size)
    }

    /// Read one on-disk data/fragment block and decompress it if needed.
    fn read_data_block(&mut self, at: u64, raw_size: u32) -> Result<Vec<u8>> {
        let len = (raw_size & SIZE_MASK) as usize;
        if len == 0 {
            return Ok(Vec::new()); // sparse
        }
        let mut raw = vec![0u8; len];
        self.src.read_at(at, &mut raw)?;
        if raw_size & SIZE_UNCOMPRESSED != 0 {
            Ok(raw)
        } else {
            decode(self.codec, &raw, self.block_size as usize)
        }
    }

    /// Read a whole file's bytes (data blocks + fragment tail), read-capped.
    fn read_file(
        &mut self,
        size: u64,
        blocks_start: u64,
        block_sizes: &[u32],
        frag_idx: u32,
        frag_off: u32,
    ) -> Result<Vec<u8>> {
        let total = checked_full_read_len(size)?;
        let mut out = Vec::with_capacity(total);
        let mut at = blocks_start;
        for &bs in block_sizes {
            if out.len() >= total {
                break;
            }
            let len = (bs & SIZE_MASK) as usize;
            if len == 0 {
                // sparse full block → zero-fill up to one block (or the remainder)
                let n = core::cmp::min(self.block_size as usize, total - out.len());
                out.resize(out.len() + n, 0);
            } else {
                let block = self.read_data_block(at, bs)?;
                at += len as u64;
                out.extend_from_slice(&block);
            }
        }
        if frag_idx != NO_FRAGMENT && out.len() < total {
            let frag = self.read_fragment(frag_idx)?;
            let start = frag_off as usize;
            let take = total - out.len();
            let tail = frag
                .get(start..start + take)
                .ok_or(FoldError::Corrupt("squashfs: fragment tail OOB"))?;
            out.extend_from_slice(tail);
        }
        out.truncate(total);
        Ok(out)
    }
}

impl<S: BlockSource> FoldFrontend<S> for SquashFs<S> {
    const TAG: &'static str = "squashfs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < 4 {
            return Ok(false);
        }
        let mut m = [0u8; 4];
        src.read_at(0, &mut m)?;
        Ok(u32::from_le_bytes(m) == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut sb = [0u8; 96];
        if src.len() < 96 {
            return Err(FoldError::Corrupt(
                "squashfs: source shorter than superblock",
            ));
        }
        let mut src = src;
        src.read_at(0, &mut sb)?;
        if le_u32(&sb, 0)? != MAGIC {
            return Err(FoldError::Corrupt("squashfs: bad magic"));
        }
        let codec = match le_u16(&sb, 20)? {
            1 => Codec::Zlib,
            3 => Codec::Lzo,
            4 => Codec::Xz,
            5 => Codec::Lz4,
            6 => Codec::Zstd,
            2 => return Err(FoldError::Unsupported("squashfs: legacy lzma1 compression")),
            _ => return Err(FoldError::Unsupported("squashfs: unknown compression id")),
        };
        let block_size = le_u32(&sb, 12)?;
        if block_size == 0 || block_size > (1 << 20) {
            return Err(FoldError::Corrupt("squashfs: implausible block size"));
        }
        let root_ref = le_u64(&sb, 32)?;
        let mut me = SquashFs {
            src,
            codec,
            block_size,
            inode_table_start: le_u64(&sb, 64)?,
            directory_table_start: le_u64(&sb, 72)?,
            fragment_table_start: le_u64(&sb, 80)?,
            nodes: Vec::new(),
            by_ref: BTreeMap::new(),
        };
        let root = me.parse_inode(root_ref)?;
        me.intern(root_ref, root); // node 0
        Ok(me)
    }

    fn root(&self) -> NodeId {
        0
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
        let inode = self.inode(dir)?;
        let Body::Dir {
            abs_block,
            offset,
            size,
        } = inode.body
        else {
            return Err(FoldError::NotDirectory);
        };
        let entries = self.dir_entries(abs_block, offset, size)?;
        let mut out = Vec::with_capacity(entries.len());
        for (name, child_ref) in entries {
            let child = self.parse_inode(child_ref)?;
            let kind = child.kind;
            let node = self.intern(child_ref, child);
            out.push(DirEntry { name, node, kind });
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
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
        let inode = self.inode(node)?;
        let Body::File {
            blocks_start,
            block_sizes,
            frag_idx,
            frag_off,
        } = &inode.body
        else {
            return Err(if inode.kind == FileKind::Directory {
                FoldError::IsDirectory
            } else {
                FoldError::Unsupported("squashfs: read of a non-regular inode")
            });
        };
        if off >= inode.size {
            return Ok(0);
        }
        // Read the whole file then slice. Streaming per-block is a follow-up.
        let data = self.read_file(inode.size, *blocks_start, block_sizes, *frag_idx, *frag_off)?;
        let start = off as usize;
        let n = core::cmp::min(buf.len(), data.len().saturating_sub(start));
        buf[..n].copy_from_slice(&data[start..start + n]);
        Ok(n)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        match self.inode(node)?.body {
            Body::Symlink(t) => Ok(Some(t)),
            _ => Ok(None),
        }
    }
}

fn le_u16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(FoldError::Corrupt("squashfs: truncated u16"))
}
fn le_u32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(FoldError::Corrupt("squashfs: truncated u32"))
}
fn le_u64(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(FoldError::Corrupt("squashfs: truncated u64"))
}
