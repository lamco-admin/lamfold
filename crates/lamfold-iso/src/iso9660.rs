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

use crate::{el_torito, rock_ridge};

/// The system area is a fixed 32 768 bytes (16 logical sectors of 2048), so the
/// Volume Descriptor Set always begins at sector 16 regardless of the volume's
/// own logical block size.
const VD_REGION_OFFSET: u64 = 16 * 2048;
const VD_SIZE: usize = 2048;
const VD_TYPE_BOOT_RECORD: u8 = 0;
const VD_TYPE_PRIMARY: u8 = 1;
const VD_TYPE_SUPPLEMENTARY: u8 = 2;
const VD_TYPE_TERMINATOR: u8 = 255;
const STANDARD_ID: &[u8; 5] = b"CD001";
/// Joliet escape sequences (at SVD offset 88): "%/@" (UCS-2 L1), "%/C" (L2),
/// "%/E" (L3). The third byte distinguishes the level; all three are Joliet.
const JOLIET_ESCAPE_PREFIX: [u8; 2] = [0x25, 0x2F]; // "%/"
const JOLIET_ESCAPE_OFFSET: usize = 88;
/// Boot System Identifier marking an El Torito boot-record volume descriptor.
const EL_TORITO_ID: &[u8] = b"EL TORITO SPECIFICATION";
/// Absolute pointer to the El Torito boot catalog (LE u32) in the boot-record VD.
const ET_CATALOG_PTR_OFFSET: usize = 71;
/// zisofs file-data header magic (8 bytes).
#[cfg(feature = "zisofs")]
const ZISOFS_MAGIC: [u8; 8] = [0x37, 0xE4, 0x53, 0x96, 0xC9, 0xDB, 0xD6, 0x07];
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
    /// zisofs parameters (Rock Ridge `ZF`), `None` for uncompressed files. When
    /// set, `size` is the *compressed* extent size and `zisofs.uncompressed_size`
    /// is the logical file size.
    zisofs: Option<rock_ridge::Zisofs>,
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
    /// This volume's chosen name tree is the Joliet SVD ⇒ decode file identifiers
    /// as UCS-2 (UTF-16BE). Mutually exclusive with `rock_ridge`.
    joliet: bool,
    /// El Torito boot catalog LBA, captured from the boot-record VD (if any).
    el_torito_catalog_lba: Option<u32>,
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

                let mut name = if self.joliet {
                    decode_name_joliet(fi, is_dir)?
                } else {
                    decode_name(fi, is_dir)?
                };
                let mut kind = if is_dir {
                    FileKind::Directory
                } else {
                    FileKind::Regular
                };
                let mut link_target = None;
                let mut zisofs = None;

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
                        zisofs = rr.zisofs;
                    }
                }
                f(
                    name,
                    Inode {
                        lba,
                        size,
                        kind,
                        link_target,
                        zisofs,
                    },
                );
            }
            pos += len;
        }
        Ok(())
    }

    /// The El Torito **UEFI** boot image as a byte-range on this source, or
    /// `None` if the volume has no UEFI boot entry. A side-channel *outside* the
    /// filesystem surface (it has no directory/inode analogue): boot-from-ISO
    /// uses it to locate the embedded UEFI loader. The byte range is
    /// `image.lba * 2048 .. + image.sectors * 512`.
    pub fn el_torito_uefi_image(&mut self) -> Result<Option<el_torito::UefiImage>> {
        let Some(cat_lba) = self.el_torito_catalog_lba else {
            return Ok(None);
        };
        let off = u64::from(cat_lba) * u64::from(self.block_size);
        if off + VD_SIZE as u64 > self.src.len() {
            return Ok(None);
        }
        let mut cat = vec![0u8; VD_SIZE];
        self.src.read_at(off, &mut cat)?;
        Ok(el_torito::parse_catalog(&cat))
    }

    /// Read a zisofs (paged-zlib) compressed file: the data extent begins with a
    /// 16-byte header + a block-pointer table; each block is zlib-compressed (a
    /// zero-length pointer span is a hole). Decompression goes through the shared
    /// substrate codec.
    #[cfg(feature = "zisofs")]
    fn read_zisofs(
        &mut self,
        inode: &Inode,
        zi: rock_ridge::Zisofs,
        off: u64,
        buf: &mut [u8],
    ) -> Result<usize> {
        use lamfold::{decode, Codec};

        let uncompressed = u64::from(zi.uncompressed_size);
        if off >= uncompressed {
            return Ok(0);
        }
        if zi.block_size_log2 >= 31 {
            return Err(FoldError::Corrupt("zisofs: implausible block size"));
        }
        let block_size = 1u64 << zi.block_size_log2;
        let n_blocks = uncompressed.div_ceil(block_size) as usize;
        let table_bytes = (n_blocks + 1)
            .checked_mul(4)
            .ok_or(FoldError::Corrupt("zisofs: block table overflow"))?;
        let head_len = checked_full_read_len(16u64 + table_bytes as u64)?;
        let extent_off = u64::from(inode.lba) * u64::from(self.block_size);
        let mut head = vec![0u8; head_len];
        self.src.read_at(extent_off, &mut head)?;
        if head[..8] != ZISOFS_MAGIC {
            return Err(FoldError::Corrupt("zisofs: bad magic"));
        }
        let ptr = |i: usize| -> u32 {
            let o = 16 + i * 4;
            u32::from_le_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]])
        };

        let want = core::cmp::min(buf.len() as u64, uncompressed - off) as usize;
        let mut produced = 0;
        let mut cur = off;
        while produced < want {
            let blk = (cur / block_size) as usize;
            let blk_base = blk as u64 * block_size;
            let this_uncomp = core::cmp::min(block_size, uncompressed - blk_base) as usize;
            let cstart = u64::from(ptr(blk));
            let cend = u64::from(ptr(blk + 1));
            let decompressed: Vec<u8> = if cend <= cstart {
                vec![0u8; this_uncomp] // a hole
            } else {
                let clen = checked_full_read_len(cend - cstart)?;
                let mut cbuf = vec![0u8; clen];
                self.src.read_at(extent_off + cstart, &mut cbuf)?;
                decode(Codec::Zlib, &cbuf, this_uncomp)?
            };
            let in_blk = (cur - blk_base) as usize;
            let avail = decompressed.len().saturating_sub(in_blk);
            let take = core::cmp::min(avail, want - produced);
            if take == 0 {
                break;
            }
            buf[produced..produced + take].copy_from_slice(&decompressed[in_blk..in_blk + take]);
            produced += take;
            cur += take as u64;
        }
        Ok(produced)
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
            joliet: false,
            el_torito_catalog_lba: None,
        };
        let mut vd = [0u8; VD_SIZE];
        let mut pvd_root: Option<Inode> = None;
        let mut joliet_root: Option<Inode> = None;
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
                    pvd_root = Some(root_record(&vd)?);
                    // Don't break — boot-record / supplementary descriptors follow.
                }
                VD_TYPE_BOOT_RECORD => {
                    if vd.get(7..7 + EL_TORITO_ID.len()) == Some(EL_TORITO_ID) {
                        me.el_torito_catalog_lba = Some(le_u32(&vd, ET_CATALOG_PTR_OFFSET)?);
                    }
                }
                VD_TYPE_SUPPLEMENTARY => {
                    // A Supplementary VD with a Joliet escape sequence carries the
                    // UCS-2 name tree.
                    let esc = &vd[JOLIET_ESCAPE_OFFSET..JOLIET_ESCAPE_OFFSET + 3];
                    if esc[..2] == JOLIET_ESCAPE_PREFIX && matches!(esc[2], 0x40 | 0x43 | 0x45) {
                        joliet_root = Some(root_record(&vd)?);
                    }
                }
                VD_TYPE_TERMINATOR => break,
                _ => continue,
            }
        }
        let pvd_root = pvd_root.ok_or(FoldError::Corrupt("iso: no primary volume descriptor"))?;

        // Name-tree preference: Rock Ridge (full POSIX) > Joliet (UCS-2) > base.
        // Rock Ridge lives on the PVD tree; detect it via the root "." SP entry.
        let root = if let Some(skip) = me.detect_susp(&pvd_root)? {
            me.rock_ridge = true;
            me.susp_skip = skip;
            pvd_root
        } else if let Some(jr) = joliet_root {
            me.joliet = true;
            jr
        } else {
            pvd_root
        };
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
        // A zisofs file's logical size is its *uncompressed* length, not the
        // compressed extent recorded in the directory record.
        let size = inode
            .zisofs
            .map_or(u64::from(inode.size), |z| u64::from(z.uncompressed_size));
        Ok(Metadata {
            kind: inode.kind,
            size,
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
        #[cfg(feature = "zisofs")]
        if let Some(zi) = inode.zisofs {
            return self.read_zisofs(&inode, zi, off, buf);
        }
        #[cfg(not(feature = "zisofs"))]
        if inode.zisofs.is_some() {
            return Err(FoldError::Unsupported(
                "zisofs-compressed file: enable the `zisofs` feature",
            ));
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

/// Extract the root directory record (offset 156, 34 bytes) of a volume
/// descriptor (PVD or Joliet SVD) as an [`Inode`].
fn root_record(vd: &[u8]) -> Result<Inode> {
    let rdr = vd
        .get(156..156 + 34)
        .ok_or(FoldError::Corrupt("iso: short root directory record"))?;
    Ok(Inode {
        lba: le_u32(rdr, dr::EXTENT_LBA_LE)?,
        size: le_u32(rdr, dr::DATA_LEN_LE)?,
        kind: FileKind::Directory,
        link_target: None,
        zisofs: None,
    })
}

/// Decode a Joliet (UCS-2 / UTF-16BE) file identifier, stripping the `;version`
/// suffix from file names.
fn decode_name_joliet(fi: &[u8], is_dir: bool) -> Result<String> {
    let units = fi.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
    let mut s = String::new();
    for ch in char::decode_utf16(units) {
        s.push(ch.map_err(|_| FoldError::InvalidPath("iso: bad UTF-16 in joliet name"))?);
    }
    if is_dir {
        Ok(s)
    } else {
        let base = s.split(';').next().unwrap_or(&s);
        let base = base.strip_suffix('.').unwrap_or(base);
        Ok(String::from(base))
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
