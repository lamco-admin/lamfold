//! UDF 1.02 reader (ECMA-167 / OSTA UDF), clean-roomed from the free specs.
//!
//! Descriptor chain: Anchor VD Pointer (sector 256) → Main Volume Descriptor
//! Sequence (Partition Descriptor → partition start; Logical Volume Descriptor →
//! block size + the File Set Descriptor long_ad) → File Set Descriptor → root
//! ICB. Inodes are File Entries (tag 261); directory entries are File Identifier
//! Descriptors; file/dir data is inline, short_ad, or long_ad.
//!
//! No `unsafe`: every on-disk field is read through bounds-checked little-endian
//! helpers; every allocation goes through the substrate read cap.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata,
    NodeId, Result, SubstrateCtx,
};

/// UDF logical sector size (the AVDP lives at "sector 256" = this × 256).
const SECTOR: u64 = 2048;
const AVDP_SECTOR: u32 = 256;
/// Descriptor tag identifiers (ECMA-167 §3 / §4).
const TAG_AVDP: u16 = 2;
const TAG_PARTITION: u16 = 5;
const TAG_LOGICAL_VOLUME: u16 = 6;
const TAG_TERMINATING: u16 = 8;
const TAG_FILE_SET: u16 = 256;
const TAG_FID: u16 = 257;
const TAG_FILE_ENTRY: u16 = 261;
const TAG_EXTENDED_FILE_ENTRY: u16 = 266;
/// ICBTag FileType values.
const FT_DIRECTORY: u8 = 4;
const FT_REGULAR: u8 = 5;
const FT_SYMLINK: u8 = 12;
const MAX_VDS_SECTORS: u32 = 64;

#[derive(Clone, Copy)]
struct Extent {
    /// Absolute LBA, or `None` for an unrecorded/sparse extent (reads as zeros).
    lba: Option<u32>,
    len: u32,
}

#[derive(Clone)]
enum FileData {
    Inline(Vec<u8>),
    Extents(Vec<Extent>),
}

#[derive(Clone)]
struct UdfInode {
    kind: FileKind,
    size: u64,
    data: FileData,
}

/// A mounted UDF volume.
pub struct Udf<S: BlockSource> {
    src: S,
    block_size: u32,
    partition_start: u32,
    nodes: Vec<UdfInode>,
    by_lba: BTreeMap<u32, NodeId>,
}

impl<S: BlockSource> Udf<S> {
    fn read_block(&mut self, lba: u32) -> Result<Vec<u8>> {
        let off = u64::from(lba) * u64::from(self.block_size);
        if off + u64::from(self.block_size) > self.src.len() {
            return Err(FoldError::Corrupt("udf: block past end of source"));
        }
        let mut b = vec![0u8; self.block_size as usize];
        self.src.read_at(off, &mut b)?;
        Ok(b)
    }

    fn intern(&mut self, fe_lba: u32, inode: UdfInode) -> NodeId {
        if let Some(&id) = self.by_lba.get(&fe_lba) {
            return id;
        }
        let id = self.nodes.len() as NodeId;
        self.nodes.push(inode);
        self.by_lba.insert(fe_lba, id);
        id
    }

    fn inode(&self, node: NodeId) -> Result<UdfInode> {
        self.nodes
            .get(node as usize)
            .cloned()
            .ok_or(FoldError::NotFound)
    }

    /// Parse a File Entry (tag 261) at `fe_lba` into an inode (kind + size + the
    /// resolved data location).
    fn read_fe(&mut self, fe_lba: u32) -> Result<UdfInode> {
        let buf = self.read_block(fe_lba)?;
        match le_u16(&buf, 0)? {
            TAG_EXTENDED_FILE_ENTRY => {
                return Err(FoldError::Unsupported(
                    "udf: Extended File Entry (UDF 2.x) not yet supported",
                ))
            }
            TAG_FILE_ENTRY => {}
            _ => return Err(FoldError::Corrupt("udf: expected a File Entry")),
        }
        let file_type = *buf.get(27).ok_or(FoldError::Corrupt("udf: short ICBTag"))?;
        let ad_type = le_u16(&buf, 34)? & 0x7;
        let info_len = le_u64(&buf, 56)?;
        let l_ea = le_u32(&buf, 168)? as usize;
        let l_ad = le_u32(&buf, 172)? as usize;
        let ad_off = 176usize
            .checked_add(l_ea)
            .ok_or(FoldError::Corrupt("udf: L_EA overflow"))?;

        let kind = match file_type {
            FT_DIRECTORY => FileKind::Directory,
            FT_REGULAR => FileKind::Regular,
            FT_SYMLINK => FileKind::Symlink,
            _ => FileKind::Other,
        };

        let data = match ad_type {
            3 => {
                // Inline: the data is the AD area itself.
                let n = checked_full_read_len(info_len)?;
                let end = ad_off
                    .checked_add(n)
                    .ok_or(FoldError::Corrupt("udf: inline overflow"))?;
                let bytes = buf
                    .get(ad_off..end)
                    .ok_or(FoldError::Corrupt("udf: inline data out of bounds"))?;
                FileData::Inline(bytes.to_vec())
            }
            0 => self.parse_extents(&buf, ad_off, l_ad, 8)?,
            1 => self.parse_extents(&buf, ad_off, l_ad, 16)?,
            _ => {
                return Err(FoldError::Unsupported(
                    "udf: extended_ad allocation descriptors not supported",
                ))
            }
        };
        Ok(UdfInode {
            kind,
            size: info_len,
            data,
        })
    }

    /// Parse a short_ad (`stride` 8) or long_ad (`stride` 16) list into extents.
    /// The extent position is partition-relative (short_ad) or carries an LBA +
    /// partition ref (long_ad); with a single physical partition both resolve to
    /// `partition_start + block`.
    fn parse_extents(
        &self,
        buf: &[u8],
        ad_off: usize,
        l_ad: usize,
        stride: usize,
    ) -> Result<FileData> {
        let ads = buf.get(ad_off..ad_off + l_ad).ok_or(FoldError::Corrupt(
            "udf: allocation descriptors out of bounds",
        ))?;
        let mut ex = Vec::new();
        for c in ads.chunks_exact(stride) {
            let raw = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            let len = raw & 0x3FFF_FFFF;
            let etype = raw >> 30;
            if len == 0 {
                continue;
            }
            if etype == 3 {
                return Err(FoldError::Unsupported(
                    "udf: allocation-extent continuation not supported",
                ));
            }
            // short_ad: position @4; long_ad: logical block @4 (partition ref @8).
            let block = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
            ex.push(Extent {
                lba: (etype == 0).then_some(self.partition_start + block),
                len,
            });
        }
        Ok(FileData::Extents(ex))
    }

    /// Read a whole file/directory's bytes (inline, or by reading its extents),
    /// capped to the inode's information length.
    fn read_all(&mut self, inode: &UdfInode) -> Result<Vec<u8>> {
        match &inode.data {
            FileData::Inline(b) => Ok(b.clone()),
            FileData::Extents(ex) => {
                let total = checked_full_read_len(inode.size)?;
                let mut out = Vec::with_capacity(total);
                for e in ex {
                    if out.len() >= total {
                        break;
                    }
                    let take = core::cmp::min(e.len as usize, total - out.len());
                    match e.lba {
                        Some(lba) => {
                            let mut chunk = vec![0u8; take];
                            self.src
                                .read_at(u64::from(lba) * u64::from(self.block_size), &mut chunk)?;
                            out.extend_from_slice(&chunk);
                        }
                        None => out.resize(out.len() + take, 0),
                    }
                }
                out.truncate(total);
                Ok(out)
            }
        }
    }
}

impl<S: BlockSource> FoldFrontend<S> for Udf<S> {
    const TAG: &'static str = "udf";

    fn probe(src: &mut S) -> Result<bool> {
        let off = u64::from(AVDP_SECTOR) * SECTOR;
        if src.len() < off + 4 {
            return Ok(false);
        }
        let mut t = [0u8; 4];
        src.read_at(off, &mut t)?;
        Ok(u16::from_le_bytes([t[0], t[1]]) == TAG_AVDP)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut me = Udf {
            src,
            block_size: SECTOR as u32,
            partition_start: 0,
            nodes: Vec::new(),
            by_lba: BTreeMap::new(),
        };

        // Anchor Volume Descriptor Pointer at sector 256 → Main VDS extent.
        let avdp = me.read_block(AVDP_SECTOR)?;
        if le_u16(&avdp, 0)? != TAG_AVDP {
            return Err(FoldError::Corrupt(
                "udf: no Anchor VD Pointer at sector 256",
            ));
        }
        let mvds_len = le_u32(&avdp, 16)?;
        let mvds_loc = le_u32(&avdp, 20)?;

        // Walk the Main VDS for the Partition + Logical Volume descriptors.
        let mut partition_start = None;
        let mut lvd = None; // (logical_block_size, fsd_lb)
        let n_sectors = (mvds_len / SECTOR as u32).min(MAX_VDS_SECTORS);
        for i in 0..n_sectors {
            let b = me.read_block(mvds_loc + i)?;
            match le_u16(&b, 0)? {
                TAG_PARTITION => partition_start = Some(le_u32(&b, 188)?),
                TAG_LOGICAL_VOLUME => {
                    let lbs = le_u32(&b, 212)?;
                    let fsd_lb = le_u32(&b, 252)?; // LogicalVolumeContentsUse long_ad
                    lvd = Some((lbs, fsd_lb));
                }
                TAG_TERMINATING => break,
                _ => {}
            }
        }
        let partition_start =
            partition_start.ok_or(FoldError::Corrupt("udf: no Partition Descriptor"))?;
        let (lbs, fsd_lb) = lvd.ok_or(FoldError::Corrupt("udf: no Logical Volume Descriptor"))?;
        me.partition_start = partition_start;
        if lbs != 0 {
            me.block_size = lbs;
        }

        // File Set Descriptor → root directory ICB.
        let fsd = me.read_block(partition_start + fsd_lb)?;
        if le_u16(&fsd, 0)? != TAG_FILE_SET {
            return Err(FoldError::Corrupt("udf: no File Set Descriptor"));
        }
        let root_icb_lb = le_u32(&fsd, 404)?; // Root Directory ICB long_ad
        let root_fe_lba = partition_start + root_icb_lb;
        let root = me.read_fe(root_fe_lba)?;
        me.intern(root_fe_lba, root); // node 0
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
        if inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let data = self.read_all(&inode)?;
        let fids = parse_fids(&data, self.partition_start)?;
        let mut out = Vec::with_capacity(fids.len());
        for (name, child_fe_lba) in fids {
            let child = self.read_fe(child_fe_lba)?;
            let kind = child.kind;
            let node = self.intern(child_fe_lba, child);
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
        if inode.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        if off >= inode.size {
            return Ok(0);
        }
        let want = core::cmp::min(buf.len() as u64, inode.size - off) as usize;
        match &inode.data {
            FileData::Inline(b) => {
                let start = off as usize;
                let n = core::cmp::min(want, b.len().saturating_sub(start));
                buf[..n].copy_from_slice(&b[start..start + n]);
                Ok(n)
            }
            FileData::Extents(ex) => {
                let mut file_pos = 0u64;
                let mut produced = 0usize;
                for e in ex {
                    if produced >= want {
                        break;
                    }
                    let ext_start = file_pos;
                    let ext_end = file_pos + u64::from(e.len);
                    file_pos = ext_end;
                    let cur = off + produced as u64;
                    if cur >= ext_end {
                        continue;
                    }
                    let intra = (cur - ext_start) as usize;
                    let avail = (e.len as usize).saturating_sub(intra);
                    let take = core::cmp::min(avail, want - produced);
                    match e.lba {
                        Some(lba) => self.src.read_at(
                            u64::from(lba) * u64::from(self.block_size) + intra as u64,
                            &mut buf[produced..produced + take],
                        )?,
                        None => buf[produced..produced + take].fill(0),
                    }
                    produced += take;
                }
                Ok(produced)
            }
        }
    }
}

/// Parse File Identifier Descriptors over a directory's data, returning
/// (name, child File Entry LBA) for each non-parent, non-deleted entry.
fn parse_fids(data: &[u8], partition_start: u32) -> Result<Vec<(String, u32)>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 38 <= data.len() {
        if le_u16(data, p)? != TAG_FID {
            break;
        }
        let fc = data[p + 18]; // FileCharacteristics: bit2 deleted, bit3 parent
        let l_fi = data[p + 19] as usize;
        let icb_lb = le_u32(data, p + 24)?; // ICB long_ad logical block
        let l_iu = le_u16(data, p + 36)? as usize;
        let fi_off = p + 38 + l_iu;
        let fi = data
            .get(fi_off..fi_off + l_fi)
            .ok_or(FoldError::Corrupt("udf: FID name out of bounds"))?;
        if fc & 0x08 == 0 && fc & 0x04 == 0 {
            out.push((decode_udf_name(fi)?, partition_start + icb_lb));
        }
        let total = (38 + l_iu + l_fi + 3) & !3; // pad to a 4-byte boundary
        if total == 0 {
            break;
        }
        p += total;
    }
    Ok(out)
}

/// Decode a UDF compressed-unicode `d-string`: a leading compression id selects
/// 8-bit (Latin-1) or 16-bit (UTF-16BE) characters.
fn decode_udf_name(fi: &[u8]) -> Result<String> {
    match fi.split_first() {
        None => Ok(String::new()),
        Some((8, rest)) => Ok(rest.iter().map(|&b| b as char).collect()),
        Some((16, rest)) => {
            let units = rest
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]));
            let mut s = String::new();
            for ch in char::decode_utf16(units) {
                s.push(ch.map_err(|_| FoldError::InvalidPath("udf: bad UTF-16 in name"))?);
            }
            Ok(s)
        }
        Some(_) => Err(FoldError::InvalidPath("udf: unknown name compression id")),
    }
}

fn le_u16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(FoldError::Corrupt("udf: truncated u16"))
}

fn le_u32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(FoldError::Corrupt("udf: truncated u32"))
}

fn le_u64(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(FoldError::Corrupt("udf: truncated u64"))
}
