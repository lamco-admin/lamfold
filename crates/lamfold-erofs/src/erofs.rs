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
    checked_block_len, checked_full_read_len, decode, lz4_block_with_dict, microlzma_block_decode,
    BlockSource, Codec, DirEntry, FileKind, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

const SUPER_OFFSET: u64 = 1024;
const MAGIC: u32 = 0xE0F5_E1E2;

// datalayout (bits 1..=3 of i_format)
const FLAT_PLAIN: u8 = 0;
const FLAT_INLINE: u8 = 2;
const COMPRESSED_FULL: u8 = 1;
// COMPRESSED_COMPACT (3) is the bit-packed `mkfs.erofs` default index.
const COMPRESSED_COMPACT: u8 = 3;

// z_erofs_map_header `h_advise` bits. COMPACTED_2B selects the 2-byte amortized
// packing within the compact index; clear ⇒ all-4B. BIG_PCLUSTER_{1,2} are set by
// default for deflate/zstd/lzma (a pcluster may span several blocks, and a NONHEAD
// carries a cblkcnt). INLINE_PCLUSTER (ztailpacking) and FRAGMENT change where the
// data lives and are refused until validated.
const ADVISE_COMPACTED_2B: u16 = 0x0001;
const ADVISE_BIG_PCLUSTER: u16 = 0x0002 | 0x0004;
const ADVISE_INLINE_PCLUSTER: u16 = 0x0008;
const ADVISE_FRAGMENT: u16 = 0x0020;

// Compact per-lcluster entry: `encodebits` = 2-bit type + `lobits` (= lclusterbits
// == 12, the only validated geometry) of value. A NONHEAD whose value has
// `CBLKCNT_FLAG` set encodes its pcluster's compressed-block count in the low bits.
const COMPACT_LOBITS: u32 = 12;
const COMPACT_ENCBITS: u32 = 14;
const COMPACT_CBLKCNT_FLAG: u16 = 0x0800;

// Head algorithm nibbles (`h_algorithmtype`).
const ALGO_LZ4: u8 = 0;
const ALGO_LZMA: u8 = 1;
const ALGO_DEFLATE: u8 = 2;
const ALGO_ZSTD: u8 = 3;

// MicroLZMA properties byte mkfs writes (lc3/lp0/pb2); the on-disk
// `z_erofs_lzma_cfgs.format` is 0 in every validated image, so props are not stored.
const MICROLZMA_PROPS: u8 = 0x5d;
// EROFS compression-config region: `available_compr_algs` bitmap, then the per-algo
// config blobs right after the 128-byte superblock.
const COMPR_ALGS_OFF: u64 = 1024 + 84;
const COMPR_CFGS_OFF: u64 = 1024 + 128;
const COMPR_ALG_LZMA_BIT: u16 = 0x0002;

// z_erofs compressed-cluster geometry. The 8-byte `z_erofs_map_header` sits at
// ALIGN(inode_end + xattr, 8); for the FULL (legacy) index the fixed-size
// `z_erofs_lcluster_index` array begins 16 bytes later (an 8-byte header plus 8
// reserved bytes — the legacy header size). Each entry's low 2 advise bits give
// the lcluster type.
const Z_EROFS_LEGACY_HEADER_SIZE: u64 = 16;
const LC_TYPE_PLAIN: u16 = 0;
const LC_TYPE_HEAD1: u16 = 1;
const LC_TYPE_NONHEAD: u16 = 2;
const LC_TYPE_HEAD2: u16 = 3;
// `lz4_max_distance` is ≤ 65535; a 64 KiB window covers every back-reference.
const LZ4_WINDOW: usize = 65_536;

struct ErofsInode {
    kind: FileKind,
    size: u64,
    mode: u16,
    layout: u8,
    raw_blkaddr: u32,
    inline_off: u64,
}

/// One pcluster, as the index walk yields it: the logical offset where its
/// decompressed output starts, its lcluster type (PLAIN/HEAD1/HEAD2), the absolute
/// block where its compressed data begins, and how many blocks that data spans
/// (1 unless big-pcluster).
struct Head {
    start: u64,
    ty: u16,
    blkaddr: u32,
    cblkcnt: u32,
}

/// A mounted EROFS volume.
pub struct Erofs<S: BlockSource> {
    src: S,
    block_size: u64,
    meta_off: u64,
    root_nid: u64,
    /// inode cache keyed by nid (parse once).
    inodes: BTreeMap<u64, ErofsInode>,
    /// fully-decoded data for compressed inodes, keyed by nid. The LZ4 sliding
    /// dictionary makes forward, whole-file decode the natural unit; cache it so
    /// repeated block reads (and verification) don't re-decompress.
    decoded: BTreeMap<u64, Vec<u8>>,
    /// MicroLZMA window from `z_erofs_lzma_cfgs`, parsed once at open. `None` when
    /// the volume declares no LZMA or an unvalidated config `format`, which makes
    /// any LZMA-head inode surface a clean `Unsupported` rather than a guess.
    lzma_dict_size: Option<u32>,
}

impl<S: BlockSource> Erofs<S> {
    fn parse_inode(&mut self, nid: u64) -> Result<()> {
        if self.inodes.contains_key(&nid) {
            return Ok(());
        }
        // `nid` is an attacker-controlled `le_u64` straight from a dirent, so guard
        // the inode-offset arithmetic — an out-of-range nid must surface as Corrupt,
        // not panic (debug / overflow-checked builds) or wrap (release).
        let off = nid
            .checked_mul(32)
            .and_then(|x| self.meta_off.checked_add(x))
            .ok_or(FoldError::Corrupt("erofs: inode offset overflow"))?;
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
        let layout = inode.layout;
        let size = inode.size;
        let raw_blkaddr = inode.raw_blkaddr;
        let inline_off = inode.inline_off;
        if off >= size {
            return Ok(0);
        }
        let end = core::cmp::min(off + buf.len() as u64, size);
        let total = (end - off) as usize;
        let bs = self.block_size;
        match layout {
            FLAT_PLAIN => {
                let base = u64::from(raw_blkaddr) * bs;
                self.src.read_at(base + off, &mut buf[..total])?;
                Ok(total)
            }
            FLAT_INLINE => {
                let full_bytes = (size / bs) * bs;
                let base = u64::from(raw_blkaddr) * bs;
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
            COMPRESSED_FULL | COMPRESSED_COMPACT => {
                self.materialize_compressed(nid)?;
                let data = self
                    .decoded
                    .get(&nid)
                    .ok_or(FoldError::Corrupt("erofs: decode cache miss"))?;
                let lo = off as usize;
                let seg = data
                    .get(lo..lo + total)
                    .ok_or(FoldError::Corrupt("erofs: decoded read past end"))?;
                buf[..total].copy_from_slice(seg);
                Ok(total)
            }
            _ => Err(FoldError::Unsupported(
                "erofs: chunk-indexed datalayout not supported",
            )),
        }
    }

    /// Parse the MicroLZMA window from `z_erofs_lzma_cfgs` once at open. Leaves
    /// `lzma_dict_size` `None` when the volume declares no LZMA or an unvalidated
    /// config `format`, so an LZMA-head inode refuses cleanly instead of guessing.
    fn parse_lzma_cfg(&mut self) {
        let mut algs = [0u8; 2];
        if self.src.read_at(COMPR_ALGS_OFF, &mut algs).is_err() {
            return;
        }
        let algs = u16::from_le_bytes(algs);
        if algs & COMPR_ALG_LZMA_BIT == 0 {
            return;
        }
        // Config blobs follow the superblock in ascending algorithm-bit order,
        // each `{ __le16 size }{ payload[size] }`. Walk to the LZMA (bit 1) blob.
        let mut cur = COMPR_CFGS_OFF;
        for alg in 0..16u32 {
            if algs & (1 << alg) == 0 {
                continue;
            }
            let mut sz = [0u8; 2];
            if self.src.read_at(cur, &mut sz).is_err() {
                return;
            }
            let size = u64::from(u16::from_le_bytes(sz));
            if alg == u32::from(ALGO_LZMA) {
                let mut pay = [0u8; 6];
                if self.src.read_at(cur + 2, &mut pay).is_err() {
                    return;
                }
                let dict = u32::from_le_bytes([pay[0], pay[1], pay[2], pay[3]]);
                let format = u16::from_le_bytes([pay[4], pay[5]]);
                // A non-zero format would put the props byte on disk (unvalidated).
                if format == 0 {
                    self.lzma_dict_size = Some(dict);
                }
                return;
            }
            cur += 2 + size;
        }
    }

    /// Decode a compressed inode (datalayout FULL or COMPACT) in full and cache it.
    /// The pcluster chain is decoded forward — LZ4 needs the prior output as a
    /// sliding dictionary — and caching the whole file backs `read_inode_data`.
    fn materialize_compressed(&mut self, nid: u64) -> Result<()> {
        if self.decoded.contains_key(&nid) {
            return Ok(());
        }
        let inode = self.inode(nid)?;
        let i_size = inode.size;
        let layout = inode.layout;
        let mh = (inode.inline_off + 7) & !7;

        // Cap i_size FIRST: every downstream size (n_lc, the compact-index
        // allocations it drives in build_heads_*, and the output buffer) is derived
        // from it, so an attacker-controlled multi-TiB i_size must be refused before
        // any allocation — not just before the final output Vec.
        let cap = checked_full_read_len(i_size)?;

        let mut hdr = [0u8; 8];
        self.src.read_at(mh, &mut hdr)?;
        let advise = u16::from_le_bytes([hdr[4], hdr[5]]);
        let head1_algo = hdr[6] & 0x0f;
        let head2_algo = (hdr[6] >> 4) & 0x0f;
        let lcbits = u32::from(hdr[7] & 7) + 12;
        let lcsize = 1u64 << lcbits;
        let n_lc = i_size.div_ceil(lcsize);

        // Refuse layout variants whose data is not where the index points.
        if advise & (ADVISE_INLINE_PCLUSTER | ADVISE_FRAGMENT) != 0 {
            return Err(FoldError::Unsupported(
                "erofs: ztailpacking / fragment pclusters not supported",
            ));
        }

        let heads = match layout {
            COMPRESSED_FULL => self.build_heads_full(mh, lcsize, n_lc)?,
            COMPRESSED_COMPACT => {
                // The compact bit-packing is validated only at lclusterbits == 12
                // and with the 2-byte-amortized packing (the mkfs default). The
                // all-4B layout (COMPACTED_2B clear) uses a different zone shape and
                // is refused until validated rather than mis-unpacked.
                if lcbits != 12 {
                    return Err(FoldError::Unsupported(
                        "erofs: compact index with lclusterbits != 12",
                    ));
                }
                if advise & ADVISE_COMPACTED_2B == 0 {
                    return Err(FoldError::Unsupported(
                        "erofs: all-4B compact index (COMPACTED_2B clear) not supported",
                    ));
                }
                self.build_heads_compact(mh + 8, lcsize, n_lc, advise)?
            }
            _ => return Err(FoldError::Corrupt("erofs: inode is not compressed")),
        };

        let mut out: Vec<u8> = Vec::with_capacity(cap);
        for k in 0..heads.len() {
            let h = &heads[k];
            let next = heads.get(k + 1).map_or(i_size, |n| n.start);
            if next < h.start {
                return Err(FoldError::Corrupt("erofs: non-monotonic pcluster starts"));
            }
            let outlen = (next - h.start) as usize;
            // mkfs emits a final HEAD/PLAIN lcluster whose clusterofs lands its
            // logical start exactly at i_size when the last pcluster boundary
            // coincides with EOF — a zero-length trailing extent. The reference
            // index walk yields this head too; it contributes no bytes, and its
            // blkaddr can point one block past the compressed data (EOF), so it
            // must not reach `read_span`. (A zero-length head is only ever the
            // last one — an interior extent always advances the logical offset.)
            if outlen == 0 {
                continue;
            }
            let algo = if h.ty == LC_TYPE_HEAD2 {
                head2_algo
            } else {
                head1_algo
            };
            let span = self.read_span(h.blkaddr, h.cblkcnt)?;
            let seg = decode_pcluster(h.ty, algo, &span, outlen, &out, self.lzma_dict_size)?;
            out.extend_from_slice(&seg);
        }
        self.decoded.insert(nid, out);
        Ok(())
    }

    /// Read a pcluster's compressed span: the `cblkcnt` contiguous blocks at
    /// `blkaddr` (one block unless big-pcluster), clamped to the device.
    fn read_span(&mut self, blkaddr: u32, cblkcnt: u32) -> Result<Vec<u8>> {
        let bs = self.block_size;
        let phys = u64::from(blkaddr) * bs;
        let want = u64::from(cblkcnt) * bs;
        let avail = core::cmp::min(want, self.src.len().saturating_sub(phys));
        if avail == 0 {
            return Err(FoldError::Corrupt("erofs: pcluster span out of range"));
        }
        let mut span = vec![0u8; avail as usize];
        self.src.read_at(phys, &mut span)?;
        Ok(span)
    }

    /// Build the pcluster list from a FULL (legacy, datalayout 1) index: a fixed
    /// 8-byte `z_erofs_lcluster_index` per lcluster, `blkaddr` read straight from
    /// each HEAD/PLAIN entry, one block per pcluster.
    fn build_heads_full(&mut self, mh: u64, lcsize: u64, n_lc: u64) -> Result<Vec<Head>> {
        let idx0 = mh + Z_EROFS_LEGACY_HEADER_SIZE;
        let mut heads = Vec::new();
        for i in 0..n_lc {
            let mut e = [0u8; 8];
            self.src.read_at(idx0 + i * 8, &mut e)?;
            let ty = u16::from_le_bytes([e[0], e[1]]) & 3;
            if ty == LC_TYPE_NONHEAD {
                continue;
            }
            let clusterofs = u64::from(u16::from_le_bytes([e[2], e[3]]));
            let blkaddr = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
            let start = if heads.is_empty() {
                0
            } else {
                i * lcsize + clusterofs
            };
            heads.push(Head {
                start,
                ty,
                blkaddr,
                cblkcnt: 1,
            });
        }
        Ok(heads)
    }

    /// Build the pcluster list from a COMPACT (datalayout 3) index: read the
    /// bit-packed region once, unpack the zone-amortized lcluster entries, then
    /// resolve each pcluster's clusterofs, cblkcnt, and absolute blkaddr.
    fn build_heads_compact(
        &mut self,
        ebase: u64,
        lcsize: u64,
        n_lc: u64,
        advise: u16,
    ) -> Result<Vec<Head>> {
        // Zone partition: an initial run of 4-byte-amortized groups aligns the
        // 2-byte-amortized packs to 32 bytes, then a final 4-byte run.
        let ebrem = (ebase & 31) as u32;
        let c4i = core::cmp::min(u64::from(((32 - ebrem) / 4) & 7), n_lc);
        let c2b = (n_lc - c4i) / 16 * 16;
        let c4f = n_lc - c4i - c2b;
        let idx_len = c4i.div_ceil(2) * 8 + c2b / 16 * 32 + c4f.div_ceil(2) * 8;
        let cap = checked_full_read_len(idx_len)?;
        let mut buf = vec![0u8; cap];
        self.src.read_at(ebase, &mut buf)?;

        let mut raw = Vec::with_capacity(n_lc as usize);
        let mut pos = 0usize;
        let mut rid = 0u32;
        read_4b_groups(&buf, &mut pos, c4i, &mut rid, &mut raw)?;
        read_2b_packs(&buf, &mut pos, c2b, &mut rid, &mut raw)?;
        read_4b_groups(&buf, &mut pos, c4f, &mut rid, &mut raw)?;
        resolve_compact_heads(&raw, lcsize, advise, ebase)
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
            decoded: BTreeMap::new(),
            lzma_dict_size: None,
        };
        me.parse_lzma_cfg();
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

/// One unpacked compact-index lcluster: its type, the 12-bit value (clusterofs for
/// HEAD/PLAIN; a back-delta or cblkcnt for NONHEAD), and the amortization region it
/// came from plus that region's per-group `u32` base blkaddr.
struct RawEntry {
    ty: u16,
    value: u16,
    region: u32,
    base: u32,
}

/// Unpack a run of 4-byte-amortized lclusters: 8-byte groups of
/// `[u16 even][u16 odd][u32 base]`, two lclusters sharing one base blkaddr.
fn read_4b_groups(
    buf: &[u8],
    pos: &mut usize,
    count: u64,
    rid: &mut u32,
    raw: &mut Vec<RawEntry>,
) -> Result<()> {
    let mut produced = 0u64;
    while produced < count {
        let g = buf
            .get(*pos..*pos + 8)
            .ok_or(FoldError::Corrupt("erofs: compact 4b group out of range"))?;
        let base = u32::from_le_bytes([g[4], g[5], g[6], g[7]]);
        for half in [
            u16::from_le_bytes([g[0], g[1]]),
            u16::from_le_bytes([g[2], g[3]]),
        ] {
            if produced >= count {
                break;
            }
            raw.push(RawEntry {
                ty: (half >> COMPACT_LOBITS) & 3,
                value: half & 0x0fff,
                region: *rid,
                base,
            });
            produced += 1;
        }
        *pos += 8;
        *rid += 1;
    }
    Ok(())
}

/// Unpack a run of 2-byte-amortized lclusters: 32-byte packs of sixteen 14-bit
/// LSB-first entries followed by the pack's `u32` base blkaddr at offset 28.
fn read_2b_packs(
    buf: &[u8],
    pos: &mut usize,
    count: u64,
    rid: &mut u32,
    raw: &mut Vec<RawEntry>,
) -> Result<()> {
    let mut produced = 0u64;
    while produced < count {
        let pack = buf
            .get(*pos..*pos + 32)
            .ok_or(FoldError::Corrupt("erofs: compact 2b pack out of range"))?;
        let base = u32::from_le_bytes([pack[28], pack[29], pack[30], pack[31]]);
        for s in 0..16u32 {
            if produced >= count {
                break;
            }
            let v = read_bits_lsb(pack, s * COMPACT_ENCBITS)?;
            raw.push(RawEntry {
                ty: (v >> COMPACT_LOBITS) as u16 & 3,
                value: (v & 0x0fff) as u16,
                region: *rid,
                base,
            });
            produced += 1;
        }
        *pos += 32;
        *rid += 1;
    }
    Ok(())
}

/// Read `COMPACT_ENCBITS` (14) bits LSB-first from `buf` at bit offset `bit_pos`.
fn read_bits_lsb(buf: &[u8], bit_pos: u32) -> Result<u32> {
    let mut v = 0u32;
    for k in 0..COMPACT_ENCBITS {
        let bp = bit_pos + k;
        let byte = *buf
            .get((bp >> 3) as usize)
            .ok_or(FoldError::Corrupt("erofs: compact bit offset out of range"))?;
        v |= u32::from((byte >> (bp & 7)) & 1) << k;
    }
    Ok(v)
}

/// Resolve the unpacked lcluster entries into pclusters: each HEAD/PLAIN opens one
/// (the first at logical 0). NONHEADs carry no extent. blkaddr resolution differs
/// by layout: non-big uses each pack's `base + 1 + (HEADs before it in the pack)`;
/// big-pcluster uses a global running sum of cblkcnt anchored at the first base.
fn resolve_compact_heads(
    raw: &[RawEntry],
    lcsize: u64,
    advise: u16,
    ebase: u64,
) -> Result<Vec<Head>> {
    let big = advise & ADVISE_BIG_PCLUSTER != 0;
    // The first amortization region is a 4B group iff the zone partition opens
    // with an initial 4B run (`compacted_4b_initial > 0`). `ebase` is always
    // 8-aligned (`mh` is ALIGN(_,8), `ebase = mh + 8`), so this matches the
    // simpler `ebase & 31 != 0` on every reachable alignment {0,8,16,24}; spell
    // it as the reference's `c4i > 0` so a non-8-aligned `ebase` (only a hostile,
    // malformed image) can never desync the anchor from the unpack.
    let first_is_4b = ((32 - (ebase & 31)) / 4) & 7 != 0;
    let first_base = raw.first().map_or(0, |e| e.base);

    let mut heads: Vec<Head> = Vec::new();
    let mut region_heads: BTreeMap<u32, u32> = BTreeMap::new();
    let mut cur_head: Option<usize> = None;
    for (i, e) in raw.iter().enumerate() {
        if e.ty == LC_TYPE_NONHEAD {
            // Big-pcluster encodes a pcluster's compressed-block count in the
            // NONHEAD whose value has the cblkcnt flag set.
            if big && e.value & COMPACT_CBLKCNT_FLAG != 0 {
                if let Some(h) = cur_head {
                    heads[h].cblkcnt = u32::from(e.value & 0x07ff);
                }
            }
            continue;
        }
        let clusterofs = u64::from(e.value);
        let start = if heads.is_empty() {
            0
        } else {
            i as u64 * lcsize + clusterofs
        };
        let blkaddr = if big {
            0 // filled by the running sum below
        } else {
            let c = region_heads.entry(e.region).or_insert(0);
            // Checked, mirroring the big-pcluster running sum below: a crafted
            // base near u32::MAX must surface Corrupt, not panic / wrap to block 0.
            let b = e
                .base
                .checked_add(1)
                .and_then(|x| x.checked_add(*c))
                .ok_or(FoldError::Corrupt("erofs: compact blkaddr overflow"))?;
            *c += 1;
            b
        };
        heads.push(Head {
            start,
            ty: e.ty,
            blkaddr,
            cblkcnt: 1,
        });
        cur_head = Some(heads.len() - 1);
    }

    if big {
        let mut blk = if first_is_4b {
            first_base
        } else {
            first_base
                .checked_add(1)
                .ok_or(FoldError::Corrupt("erofs: compact base overflow"))?
        };
        for k in 0..heads.len() {
            if k > 0 {
                blk = blk
                    .checked_add(heads[k - 1].cblkcnt)
                    .ok_or(FoldError::Corrupt("erofs: compact blkaddr overflow"))?;
            }
            heads[k].blkaddr = blk;
        }
    }
    Ok(heads)
}

/// Decode one pcluster's `span` (its `cblkcnt` blocks) to `outlen` bytes. PLAIN
/// pclusters are raw-stored (left-aligned); HEAD pclusters dispatch on the head
/// algorithm. `prior_out` is the decoded-so-far output (LZ4's sliding dictionary).
fn decode_pcluster(
    ty: u16,
    algo: u8,
    span: &[u8],
    outlen: usize,
    prior_out: &[u8],
    lzma_dict: Option<u32>,
) -> Result<Vec<u8>> {
    match ty {
        LC_TYPE_PLAIN => {
            // Apply the same per-block decompression-bomb cap the codec arms get
            // via `decode` — a PLAIN copy is raw, but `outlen` is still attacker-
            // controlled and must not allocate past the 16 MiB block cap.
            let _ = checked_block_len(outlen as u64)?;
            span.get(..outlen)
                .map(<[u8]>::to_vec)
                .ok_or(FoldError::Corrupt(
                    "erofs: plain pcluster shorter than output",
                ))
        }
        LC_TYPE_HEAD1 | LC_TYPE_HEAD2 => decode_head(algo, span, outlen, prior_out, lzma_dict),
        _ => Err(FoldError::Corrupt("erofs: non-head pcluster in decode")),
    }
}

/// Decode a HEAD pcluster by its algorithm. LZ4 fills the (single) block and reads
/// the sliding-window dictionary; deflate/zstd/lzma are self-contained,
/// right-aligned streams spanning `cblkcnt` blocks (no cross-pcluster dictionary).
fn decode_head(
    algo: u8,
    span: &[u8],
    outlen: usize,
    prior_out: &[u8],
    lzma_dict: Option<u32>,
) -> Result<Vec<u8>> {
    match algo {
        ALGO_LZ4 => {
            let win = &prior_out[prior_out.len().saturating_sub(LZ4_WINDOW)..];
            lz4_pcluster(span, outlen, win)
        }
        ALGO_DEFLATE => deflate_pcluster(span, outlen),
        ALGO_ZSTD => zstd_pcluster(span, outlen),
        ALGO_LZMA => {
            let dict = lzma_dict.ok_or(FoldError::Unsupported("erofs: lzma config unavailable"))?;
            lzma_pcluster(span, outlen, dict)
        }
        _ => Err(FoldError::Unsupported(
            "erofs: unknown compressed head algorithm",
        )),
    }
}

/// LZ4 pcluster. Like the other codecs, mkfs right-aligns the lz4 stream within
/// the pcluster's blocks on a short/partial-final pcluster: the stream occupies
/// the last `complen` bytes and any leading bytes are zero padding. A full
/// pcluster is left-aligned (start 0). lz4 self-terminates at `outlen`, so a
/// wrong start either errors or yields the wrong length — accept the start (0,
/// else the first non-zero byte) whose dict-decode produces exactly `outlen`
/// bytes. (`win` is the sliding-window dictionary: the prior decoded output.)
fn lz4_pcluster(span: &[u8], outlen: usize, win: &[u8]) -> Result<Vec<u8>> {
    let fnz = span.iter().position(|&b| b != 0).unwrap_or(0);
    for start in [0usize, fnz] {
        let tail = span.get(start..).unwrap_or(&[]);
        if let Ok(v) = lz4_block_with_dict(tail, outlen, win) {
            if v.len() == outlen {
                return Ok(v);
            }
        }
        if start == fnz {
            break;
        }
    }
    Err(FoldError::Corrupt("erofs lz4: no valid stream start"))
}

/// Raw-DEFLATE pcluster. The stream ends at the span's last byte; any leading
/// bytes (on a short/partial-final pcluster) are zero padding. A full pcluster
/// starts at offset 0; a padded one at its first non-zero byte. Accept the start
/// whose bounded inflate yields exactly `outlen` bytes.
fn deflate_pcluster(span: &[u8], outlen: usize) -> Result<Vec<u8>> {
    let fnz = span.iter().position(|&b| b != 0).unwrap_or(0);
    let mut last = FoldError::Corrupt("erofs deflate: no valid stream start");
    for start in [0usize, fnz] {
        let tail = span.get(start..).unwrap_or(&[]);
        match decode(Codec::Deflate, tail, outlen) {
            Ok(v) if v.len() == outlen => return Ok(v),
            Ok(_) => {}
            // A disabled-codec / bomb-cap refusal is the real cause — surface it
            // rather than masking it as a generic "no valid stream start".
            Err(e @ (FoldError::Unsupported(_) | FoldError::FileTooLarge { .. })) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// ZSTD pcluster: one full frame, right-aligned to the span. The frame start is
/// the unique offset of the magic (zero padding can never form it).
fn zstd_pcluster(span: &[u8], outlen: usize) -> Result<Vec<u8>> {
    const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
    let start = span
        .windows(4)
        .position(|w| w == MAGIC)
        .ok_or(FoldError::Corrupt("erofs zstd: frame magic not found"))?;
    decode(Codec::Zstd, &span[start..], outlen)
}

/// MicroLZMA pcluster: a raw LZMA1 stream right-aligned to the span. Its first
/// non-zero byte is the discard slot (mkfs writes the constant `0xA2`);
/// `microlzma_block_decode` swaps it for LZMA1's mandatory leading `0x00`.
fn lzma_pcluster(span: &[u8], outlen: usize, dict_size: u32) -> Result<Vec<u8>> {
    let start = span
        .iter()
        .position(|&b| b != 0)
        .ok_or(FoldError::Corrupt("erofs lzma: all-zero span"))?;
    microlzma_block_decode(&span[start..], MICROLZMA_PROPS, dict_size, outlen)
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
