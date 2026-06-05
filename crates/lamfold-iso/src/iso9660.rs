//! ISO9660 / ECMA-119 base reader (the foundation of the optical frontend).
//!
//! Clean-roomed from the public ECMA-119 specification. Reads the Primary Volume
//! Descriptor, walks Directory Records, and reads file extents over a lamfold
//! [`BlockSource`]. Rock Ridge (real names/symlinks/POSIX), Joliet (UCS-2
//! names), El Torito (boot catalog), and zisofs (transparent Deflate) are
//! layered on separately; this module is the base every one of them extends.
//!
//! No `unsafe`: every on-disk field is read through bounds-checked little-endian
//! helpers (ISO9660 stores integers "both-endian" — LE followed by BE; we read
//! the LE half), and every allocation goes through the substrate's read cap.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata,
    NodeId, Result, SubstrateCtx,
};

use crate::rock_ridge;

/// The system area is a fixed 32 768 bytes (16 logical sectors of 2048), so the
/// Volume Descriptor Set always begins at sector 16 regardless of the volume's
/// own logical block size.
const VD_REGION_OFFSET: u64 = 16 * 2048;
const VD_SIZE: usize = 2048;
const VD_TYPE_PRIMARY: u8 = 1;
const VD_TYPE_TERMINATOR: u8 = 255;
const STANDARD_ID: &[u8; 5] = b"CD001";
/// Bound on the descriptor scan — a real volume has a handful, never this many.
const MAX_VD_SCAN: u64 = 64;

/// Directory-record field offsets (ECMA-119 §9.1).
mod dr {
    pub const EXTENT_LBA_LE: usize = 2; // both-endian u32, LE half
    pub const DATA_LEN_LE: usize = 10; // both-endian u32, LE half
    pub const FILE_FLAGS: usize = 25;
    pub const LEN_FI: usize = 32;
    pub const FILE_ID: usize = 33;
    pub const FLAG_DIRECTORY: u8 = 0x02;
}

#[derive(Clone)]
struct Inode {
    lba: u32,
    size: u32,
    kind: FileKind,
    /// Symlink target bytes (Rock Ridge `SL`), `None` for non-symlinks.
    link_target: Option<Vec<u8>>,
}

/// A mounted ISO9660 volume.
pub struct Iso9660<S: BlockSource> {
    src: S,
    block_size: u32,
    nodes: Vec<Inode>,
    /// Intern table: extent LBA → node index, so repeated walks don't grow the
    /// node list without bound.
    by_lba: BTreeMap<u32, NodeId>,
    /// Rock Ridge present on this volume (SUSP `SP` found in the root).
    rock_ridge: bool,
    /// SUSP skip length (bytes to ignore at the start of each System Use area).
    susp_skip: usize,
}

impl<S: BlockSource> Iso9660<S> {
    fn intern(&mut self, inode: Inode) -> NodeId {
        if let Some(&id) = self.by_lba.get(&inode.lba) {
            return id;
        }
        let lba = inode.lba;
        let id = self.nodes.len() as NodeId;
        self.nodes.push(inode);
        self.by_lba.insert(lba, id);
        id
    }

    fn inode(&self, node: NodeId) -> Result<Inode> {
        self.nodes
            .get(node as usize)
            .cloned()
            .ok_or(FoldError::NotFound)
    }

    /// Read a directory's whole extent into memory (read-capped).
    fn read_dir_extent(&mut self, inode: Inode) -> Result<Vec<u8>> {
        let len = checked_full_read_len(u64::from(inode.size))?;
        let mut buf = vec![0u8; len];
        let off = u64::from(inode.lba) * u64::from(self.block_size);
        self.src.read_at(off, &mut buf)?;
        Ok(buf)
    }

    /// Read the root directory's "." record and detect the SUSP `SP` indicator,
    /// returning the per-record skip length if Rock Ridge is present.
    fn detect_susp(&mut self, root: &Inode) -> Result<Option<usize>> {
        let buf = self.read_dir_extent(root.clone())?;
        if buf.is_empty() {
            return Ok(None);
        }
        let len = buf[0] as usize;
        let rec = buf
            .get(..len)
            .ok_or(FoldError::Corrupt("iso: short root record"))?;
        let fi_len = *rec
            .get(dr::LEN_FI)
            .ok_or(FoldError::Corrupt("iso: short root record"))? as usize;
        let pad = usize::from(fi_len % 2 == 0);
        let su_start = dr::FILE_ID + fi_len + pad;
        let su = rec.get(su_start..).unwrap_or(&[]);
        Ok(rock_ridge::detect_sp(su))
    }

    /// Iterate the records in a directory extent, invoking `f` for each non-dot
    /// entry with (name, child Inode).
    fn for_each_entry(&mut self, dir: Inode, mut f: impl FnMut(String, Inode)) -> Result<()> {
        let buf = self.read_dir_extent(dir)?;
        let bs = self.block_size as usize;
        let mut pos = 0usize;
        while pos < buf.len() {
            let len = buf[pos] as usize;
            if len == 0 {
                // Padding to the next logical-sector boundary.
                let next = (pos / bs + 1) * bs;
                if next <= pos {
                    break;
                }
                pos = next;
                continue;
            }
            let rec = buf
                .get(pos..pos + len)
                .ok_or(FoldError::Corrupt("iso: directory record overruns extent"))?;
            let fi_len = *rec
                .get(dr::LEN_FI)
                .ok_or(FoldError::Corrupt("iso: short record"))? as usize;
            let fi = rec
                .get(dr::FILE_ID..dr::FILE_ID + fi_len)
                .ok_or(FoldError::Corrupt("iso: file id overruns record"))?;
            // Skip "." (0x00) and ".." (0x01).
            let is_dot = fi_len == 1 && (fi[0] == 0x00 || fi[0] == 0x01);
            if !is_dot {
                let flags = *rec
                    .get(dr::FILE_FLAGS)
                    .ok_or(FoldError::Corrupt("iso: short flags"))?;
                let is_dir = flags & dr::FLAG_DIRECTORY != 0;
                let lba = le_u32(rec, dr::EXTENT_LBA_LE)?;
                let size = le_u32(rec, dr::DATA_LEN_LE)?;

                let mut name = decode_name(fi, is_dir)?;
                let mut kind = if is_dir {
                    FileKind::Directory
                } else {
                    FileKind::Regular
                };
                let mut link_target = None;

                if self.rock_ridge {
                    // System Use area: after the file id (+ a pad byte when LEN_FI
                    // is even), minus the SUSP skip length.
                    let pad = usize::from(fi_len % 2 == 0);
                    let su_start = dr::FILE_ID + fi_len + pad;
                    if let Some(su) = rec.get(su_start..) {
                        let su = su.get(self.susp_skip..).unwrap_or(&[]);
                        let rr = rock_ridge::parse_su(su);
                        if let Some(n) = rr.name {
                            name = n;
                        }
                        if let Some(target) = rr.symlink_target {
                            kind = FileKind::Symlink;
                            link_target = Some(target);
                        }
                    }
                }
                f(
                    name,
                    Inode {
                        lba,
                        size,
                        kind,
                        link_target,
                    },
                );
            }
            pos += len;
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Iso9660<S> {
    const TAG: &'static str = "iso9660";

    fn probe(src: &mut S) -> Result<bool> {
        let mut hdr = [0u8; 8];
        if src.len() < VD_REGION_OFFSET + hdr.len() as u64 {
            return Ok(false);
        }
        src.read_at(VD_REGION_OFFSET, &mut hdr)?;
        // "CD001" at offset 1 of the first volume descriptor.
        Ok(&hdr[1..6] == STANDARD_ID)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut me = Iso9660 {
            src,
            block_size: 2048,
            nodes: Vec::new(),
            by_lba: BTreeMap::new(),
            rock_ridge: false,
            susp_skip: 0,
        };
        let mut vd = [0u8; VD_SIZE];
        let mut root: Option<Inode> = None;
        for i in 0..MAX_VD_SCAN {
            let off = VD_REGION_OFFSET + i * VD_SIZE as u64;
            if off + VD_SIZE as u64 > me.src.len() {
                break;
            }
            me.src.read_at(off, &mut vd)?;
            if &vd[1..6] != STANDARD_ID {
                return Err(FoldError::Corrupt("iso: missing CD001 standard id"));
            }
            match vd[0] {
                VD_TYPE_PRIMARY => {
                    // Logical block size: both-endian u16 at offset 128 (LE half).
                    me.block_size = u32::from(le_u16(&vd, 128)?);
                    if me.block_size == 0 {
                        return Err(FoldError::Corrupt("iso: zero logical block size"));
                    }
                    // Root directory record: 34 bytes at offset 156.
                    let rdr = &vd[156..156 + 34];
                    root = Some(Inode {
                        lba: le_u32(rdr, dr::EXTENT_LBA_LE)?,
                        size: le_u32(rdr, dr::DATA_LEN_LE)?,
                        kind: FileKind::Directory,
                        link_target: None,
                    });
                    break;
                }
                VD_TYPE_TERMINATOR => break,
                _ => continue, // boot record, supplementary (Joliet — S1-cont), etc.
            }
        }
        let root = root.ok_or(FoldError::Corrupt("iso: no primary volume descriptor"))?;
        // Detect SUSP/Rock Ridge: the root directory's "." record carries the
        // `SP` indicator + the per-record skip length.
        if let Some(skip) = me.detect_susp(&root)? {
            me.rock_ridge = true;
            me.susp_skip = skip;
        }
        me.intern(root); // node 0 = root
        Ok(me)
    }

    fn root(&self) -> NodeId {
        0
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        let dir_inode = self.inode(dir)?;
        if dir_inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let mut found: Option<Inode> = None;
        self.for_each_entry(dir_inode, |n, inode| {
            if found.is_none() && n == name {
                found = Some(inode);
            }
        })?;
        Ok(found.map(|i| self.intern(i)))
    }

    fn read_dir(&mut self, dir: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let dir_inode = self.inode(dir)?;
        if dir_inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let mut collected: Vec<(String, Inode)> = Vec::new();
        self.for_each_entry(dir_inode, |n, inode| collected.push((n, inode)))?;
        let mut out = Vec::with_capacity(collected.len());
        for (name, inode) in collected {
            let kind = inode.kind;
            let node = self.intern(inode);
            out.push(DirEntry { name, node, kind });
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind,
            size: u64::from(inode.size),
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
        let size = u64::from(inode.size);
        if off >= size {
            return Ok(0);
        }
        let n = core::cmp::min(buf.len() as u64, size - off) as usize;
        // Base ISO9660 files are a single contiguous extent (multi-extent is an
        // extension handled separately).
        let at = u64::from(inode.lba) * u64::from(self.block_size) + off;
        self.src.read_at(at, &mut buf[..n])?;
        Ok(n)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        Ok(self.inode(node)?.link_target)
    }
}

/// Read the little-endian half of a "both-endian" u16 (ECMA-119 §7.2.3).
fn le_u16(b: &[u8], off: usize) -> Result<u16> {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(FoldError::Corrupt("iso: truncated u16 field"))
}

/// Read the little-endian half of a "both-endian" u32 (ECMA-119 §7.3.3).
fn le_u32(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(FoldError::Corrupt("iso: truncated u32 field"))
}

/// Decode an ISO9660 file identifier: ASCII d-characters, with the `;version`
/// suffix stripped from file names (directories carry no version). Rock Ridge /
/// Joliet supply real-case / Unicode names separately; the base name is the raw
/// (uppercase) identifier.
fn decode_name(fi: &[u8], is_dir: bool) -> Result<String> {
    let raw =
        core::str::from_utf8(fi).map_err(|_| FoldError::InvalidPath("iso: non-utf8 file id"))?;
    let name = if is_dir {
        raw
    } else {
        // Strip ";version"; also drop a trailing '.' on extensionless names.
        let base = raw.split(';').next().unwrap_or(raw);
        base.strip_suffix('.').unwrap_or(base)
    };
    Ok(String::from(name))
}
