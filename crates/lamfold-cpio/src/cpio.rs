//! A `FoldFrontend` tree over a cpio archive, or an initramfs made of several.
//!
//! An initramfs is often several cpio archives back to back (an uncompressed
//! early archive with CPU microcode, then the main one), each ending in a
//! `TRAILER!!!` record and padded with zeros, and the kernel unpacks them all.
//! The newc and CRC formats the kernel accepts are parsed here, archive after
//! archive; the older odc and binary formats go through the reused
//! `cpio_reader` parser as a single archive.
//!
//! The whole input is read into memory, the flat path list is folded into a
//! directory tree (creating intermediate directories that a record implies),
//! and each file or symlink is stored as an `(offset, len)` view into the
//! input, so reads are zero-copy slices.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;

use lamfold::{
    checked_full_read_len, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata,
    NodeId, Result, SubstrateCtx,
};

const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;

/// newc (`070701`) and CRC (`070702`) headers: the magic, then 13 fields of 8
/// hex digits.
const NEWC_HEADER: usize = 110;
const NEWC_MAGIC: &[u8] = b"070701";
const CRC_MAGIC: &[u8] = b"070702";
const TRAILER: &[u8] = b"TRAILER!!!";
/// The kernel's limit on a name (`PATH_MAX`).
const MAX_NAME: usize = 4096;

/// What an initramfs member compressed by the kernel's decompressors starts
/// with: gzip, bzip2, LZMA, xz, LZO, LZ4 (legacy frame) and zstd.
const COMPRESSED_MAGICS: &[&[u8]] = &[
    &[0x1f, 0x8b],
    b"BZh",
    &[0x5d, 0x00, 0x00],
    &[0xfd, b'7', b'z', b'X', b'Z', 0x00],
    &[0x89, b'L', b'Z', b'O'],
    &[0x02, 0x21, 0x4c, 0x18],
    &[0x28, 0xb5, 0x2f, 0xfd],
];

/// One newc or CRC record.
struct Record {
    name: (usize, usize),
    mode: u32,
    /// `(devmajor, devminor, ino)`: a hard link's identity within one archive.
    inode: (u32, u32, u32),
    nlink: u32,
    data: (usize, usize),
}

const fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Parse the 8 hex digits of header field `i` of the header at `at`.
fn hex_field(archive: &[u8], at: usize, i: usize) -> Result<u32> {
    let start = at + 6 + i * 8;
    let digits = archive
        .get(start..start + 8)
        .ok_or(FoldError::Corrupt("cpio_short_header"))?;
    digits.iter().try_fold(0u32, |acc, &d| {
        let v = char::from(d)
            .to_digit(16)
            .ok_or(FoldError::Corrupt("cpio_bad_header"))?;
        Ok((acc << 4) | v)
    })
}

/// Parse the record at `at` (its magic already checked). Returns the record
/// and the offset of the next one.
fn parse_record(archive: &[u8], at: usize, crc: bool) -> Result<(Record, usize)> {
    let field = |i| hex_field(archive, at, i);
    let (ino, mode, nlink, filesize) = (field(0)?, field(1)?, field(4)?, field(6)?);
    let (devmajor, devminor, namesize, check) = (field(7)?, field(8)?, field(11)?, field(12)?);
    let namesize = namesize as usize;
    if namesize == 0 || namesize > MAX_NAME {
        return Err(FoldError::Corrupt("cpio_bad_namesize"));
    }
    let name_start = at + NEWC_HEADER;
    // The name ends in a NUL counted by namesize.
    let name_end = name_start + namesize - 1;
    if archive.get(name_end) != Some(&0) {
        return Err(FoldError::Corrupt("cpio_bad_name"));
    }
    let data_start = align4(name_start + namesize);
    let data_end = data_start
        .checked_add(filesize as usize)
        .filter(|&e| e <= archive.len())
        .ok_or(FoldError::Corrupt("cpio_short_data"))?;
    // The CRC format's check is the byte sum of a regular file's data; GNU
    // cpio leaves it 0 for symlinks, and the kernel checks regular files only.
    if crc && mode & S_IFMT == S_IFREG {
        let sum = archive[data_start..data_end]
            .iter()
            .fold(0u32, |a, &b| a.wrapping_add(u32::from(b)));
        if sum != check {
            return Err(FoldError::Corrupt("cpio_bad_crc"));
        }
    }
    let record = Record {
        name: (name_start, name_end - name_start),
        mode,
        inode: (devmajor, devminor, ino),
        nlink,
        data: (data_start, filesize as usize),
    };
    Ok((record, align4(data_end)))
}

struct Node {
    kind: FileKind,
    /// `(offset, len)` into `archive` for files/symlinks; `(0, 0)` for dirs.
    data: (usize, usize),
    mode: u32,
    children: BTreeMap<String, NodeId>,
}

impl Node {
    fn dir(mode: u32) -> Self {
        Node {
            kind: FileKind::Directory,
            data: (0, 0),
            mode,
            children: BTreeMap::new(),
        }
    }
}

/// A mounted cpio archive presented as a read-only tree.
///
/// The archive is read into memory at `open`, so the source `S` is not retained;
/// `PhantomData<S>` keeps the type monomorphic per source (the frontend pattern
/// the dispatcher expects) without storing one.
pub struct Cpio<S> {
    archive: Vec<u8>,
    nodes: Vec<Node>,
    _src: PhantomData<S>,
}

impl<S> Cpio<S> {
    fn node(&self, id: NodeId) -> Result<&Node> {
        self.nodes.get(id as usize).ok_or(FoldError::NotFound)
    }

    /// Fold every newc/CRC archive in an initramfs into the tree, as the kernel
    /// unpacks them: Zero padding between archives is skipped four bytes at a
    /// time, a later archive's file replaces an earlier one at the same path,
    /// and anything else between archives is refused rather than ignored.
    fn insert_newc_chain(&mut self, archive: &[u8]) -> Result<()> {
        let mut pos = 0;
        loop {
            while archive.get(pos..pos + 4) == Some(&[0u8; 4][..]) {
                pos += 4;
            }
            let rest = &archive[pos.min(archive.len())..];
            if rest.iter().all(|&b| b == 0) {
                return Ok(());
            }
            if COMPRESSED_MAGICS.iter().any(|m| rest.starts_with(m)) {
                // Reading it needs a streaming decoder in lamfold.
                return Err(FoldError::Unsupported("cpio_compressed_member"));
            }
            let crc = rest.starts_with(CRC_MAGIC);
            if !crc && !rest.starts_with(NEWC_MAGIC) {
                return Err(FoldError::Corrupt("cpio_junk_between_archives"));
            }
            pos = self.insert_newc_archive(archive, pos)?;
        }
    }

    /// Fold one newc/CRC archive starting at `start` into the tree; returns
    /// the offset after its trailer.
    fn insert_newc_archive(&mut self, archive: &[u8], start: usize) -> Result<usize> {
        let mut records = Vec::new();
        let mut pos = start;
        loop {
            let magic = archive
                .get(pos..pos + 6)
                .ok_or(FoldError::Corrupt("cpio_no_trailer"))?;
            let crc = magic == CRC_MAGIC;
            if !crc && magic != NEWC_MAGIC {
                return Err(FoldError::Corrupt("cpio_bad_magic"));
            }
            let (record, next) = parse_record(archive, pos, crc)?;
            pos = next;
            let (name_off, name_len) = record.name;
            if &archive[name_off..name_off + name_len] == TRAILER {
                break;
            }
            records.push(record);
        }

        // newc stores a hard-linked file's data once, with the last of its
        // links, and the others have none; every link gets that data.
        let mut linked: BTreeMap<(u32, u32, u32), (usize, usize)> = BTreeMap::new();
        for r in &records {
            if r.mode & S_IFMT == S_IFREG && r.nlink > 1 && r.data.1 > 0 {
                linked.insert(r.inode, r.data);
            }
        }
        for r in &records {
            let name = core::str::from_utf8(&archive[r.name.0..r.name.0 + r.name.1])
                .map_err(|_| FoldError::Unsupported("cpio_name_not_utf8"))?;
            let kind = match r.mode & S_IFMT {
                S_IFDIR => FileKind::Directory,
                S_IFREG => FileKind::Regular,
                S_IFLNK => FileKind::Symlink,
                _ => FileKind::Other,
            };
            let data = if kind == FileKind::Regular && r.nlink > 1 {
                linked.get(&r.inode).copied().unwrap_or(r.data)
            } else {
                r.data
            };
            self.insert(name, kind, r.mode & 0o7777, data);
        }
        Ok(pos)
    }

    /// Fold a single odc or binary archive into the tree (formats the kernel
    /// doesn't take for an initramfs, so never chained).
    fn insert_single(&mut self, archive: &[u8]) {
        let base = archive.as_ptr() as usize;
        for entry in cpio_reader::iter_files(archive) {
            let kind = match entry.mode().bits() & S_IFMT {
                S_IFDIR => FileKind::Directory,
                S_IFREG => FileKind::Regular,
                S_IFLNK => FileKind::Symlink,
                _ => FileKind::Other,
            };
            let file = entry.file();
            let off = file.as_ptr() as usize - base;
            self.insert(
                entry.name(),
                kind,
                entry.mode().bits() & 0o7777,
                (off, file.len()),
            );
        }
    }

    /// Fold one archive record into the tree at its path.
    fn insert(&mut self, path: &str, kind: FileKind, mode: u32, data: (usize, usize)) {
        let comps: Vec<&str> = path
            .split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .collect();
        let mut cur = 0usize;
        for (i, comp) in comps.iter().enumerate() {
            let last = i + 1 == comps.len();
            if let Some(&existing) = self.nodes[cur].children.get(*comp) {
                cur = existing as usize;
                // an explicit record can fill in a node first implied by a child
                if last && kind != FileKind::Directory {
                    self.nodes[cur].kind = kind;
                    self.nodes[cur].data = data;
                    self.nodes[cur].mode = mode;
                }
                continue;
            }
            let id = self.nodes.len();
            self.nodes.push(if last {
                Node {
                    kind,
                    data,
                    mode,
                    children: BTreeMap::new(),
                }
            } else {
                Node::dir(0o755)
            });
            self.nodes[cur]
                .children
                .insert((*comp).to_string(), id as NodeId);
            cur = id;
        }
    }
}

impl<S: BlockSource> FoldFrontend<S> for Cpio<S> {
    const TAG: &'static str = "cpio";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < 6 {
            return Ok(false);
        }
        let mut m = [0u8; 6];
        src.read_at(0, &mut m)?;
        // newc (070701), CRC (070702), portable ASCII (070707), or old binary.
        let ascii = matches!(&m, b"070701" | b"070702" | b"070707");
        let binary = m[0] == 0o161 && m[1] == 0o307 || m[0] == 0o307 && m[1] == 0o161;
        Ok(ascii || binary)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut src = src;
        let len = checked_full_read_len(src.len())?;
        let mut archive = vec![0u8; len];
        src.read_at(0, &mut archive)?;

        let mut me = Cpio {
            nodes: vec![Node::dir(0o755)], // node 0 = root
            archive: Vec::new(),
            _src: PhantomData,
        };
        if archive.starts_with(NEWC_MAGIC) || archive.starts_with(CRC_MAGIC) {
            me.insert_newc_chain(&archive)?;
        } else {
            me.insert_single(&archive);
        }
        me.archive = archive;
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
        let node = self.node(dir)?;
        if node.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        Ok(node.children.get(name).copied())
    }

    fn read_dir(&mut self, dir: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let node = self.node(dir)?;
        if node.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let mut out = Vec::with_capacity(node.children.len());
        for (name, &child) in &node.children {
            out.push(DirEntry {
                name: name.clone(),
                node: child,
                kind: self.nodes[child as usize].kind,
            });
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let n = self.node(node)?;
        Ok(Metadata {
            kind: n.kind,
            size: if n.kind == FileKind::Directory {
                0
            } else {
                n.data.1 as u64
            },
            mode: n.mode,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        _cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let n = self.node(node)?;
        if n.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        let (data_off, data_len) = n.data;
        if off >= data_len as u64 {
            return Ok(0);
        }
        let start = data_off + off as usize;
        let count = core::cmp::min(buf.len(), data_len - off as usize);
        buf[..count].copy_from_slice(&self.archive[start..start + count]);
        Ok(count)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let n = self.node(node)?;
        if n.kind != FileKind::Symlink {
            return Ok(None);
        }
        let (off, len) = n.data;
        Ok(Some(self.archive[off..off + len].to_vec()))
    }
}
