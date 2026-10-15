//! A `FoldFrontend` tree over a cpio archive, or an initramfs made of several.
//!
//! An initramfs is often several cpio archives back to back (an uncompressed
//! early archive with CPU microcode, then the main one, usually compressed),
//! each ending in a `TRAILER!!!` record and padded with zeros, and the kernel
//! unpacks them all. The newc and CRC formats the kernel accepts are parsed
//! here, archive after archive; a compressed member is decoded with
//! [`lamfold::decode_stream`] and its archives parsed the same way. The older
//! odc and binary formats go through the reused `cpio_reader` parser as a
//! single archive.
//!
//! The whole input is read into memory, each decompressed member is kept as
//! its own buffer, the flat path list is folded into a directory tree
//! (creating intermediate directories that a record implies), and each file or
//! symlink is stored as a span of one buffer, so reads are zero-copy slices.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;

use lamfold::{
    checked_full_read_len, decode_stream, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend,
    Metadata, NodeId, Result, StreamCodec, SubstrateCtx, MAX_BOOT_FILE_BYTES,
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

/// Where a file's or symlink's bytes are: `len` bytes at `off` in buffer
/// `buf` (0 is the input, then one buffer per decompressed member).
#[derive(Clone, Copy, Default)]
struct Span {
    buf: usize,
    off: usize,
    len: usize,
}

struct Node {
    kind: FileKind,
    /// Empty for directories.
    data: Span,
    mode: u32,
    children: BTreeMap<String, NodeId>,
}

impl Node {
    fn dir(mode: u32) -> Self {
        Node {
            kind: FileKind::Directory,
            data: Span::default(),
            mode,
            children: BTreeMap::new(),
        }
    }
}

fn file_kind(mode: u32) -> FileKind {
    match mode & S_IFMT {
        S_IFDIR => FileKind::Directory,
        S_IFREG => FileKind::Regular,
        S_IFLNK => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

/// Fold one archive record into the tree at its path.
fn insert(nodes: &mut Vec<Node>, path: &str, kind: FileKind, mode: u32, data: Span) {
    let comps: Vec<&str> = path
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    let mut cur = 0usize;
    for (i, comp) in comps.iter().enumerate() {
        let last = i + 1 == comps.len();
        if let Some(&existing) = nodes[cur].children.get(*comp) {
            cur = existing as usize;
            // an explicit record can fill in a node first implied by a child
            if last && kind != FileKind::Directory {
                nodes[cur].kind = kind;
                nodes[cur].data = data;
                nodes[cur].mode = mode;
            }
            continue;
        }
        let id = nodes.len();
        nodes.push(if last {
            Node {
                kind,
                data,
                mode,
                children: BTreeMap::new(),
            }
        } else {
            Node::dir(0o755)
        });
        nodes[cur]
            .children
            .insert((*comp).to_string(), id as NodeId);
        cur = id;
    }
}

/// Fold every archive in `buffers[idx]` into the tree, as the kernel unpacks
/// an initramfs: Zero bytes between archives are skipped, an archive starts at
/// a 4-byte-aligned offset, a later archive's file replaces an earlier one at
/// the same path, a compressed member is decoded and its archives folded in,
/// and anything else between archives is refused rather than ignored.
/// `budget` is what's left of the decompressed total ([`MAX_BOOT_FILE_BYTES`]).
fn insert_chain(
    buffers: &mut Vec<Vec<u8>>,
    nodes: &mut Vec<Node>,
    idx: usize,
    budget: &mut usize,
) -> Result<()> {
    let mut pos = 0;
    loop {
        let archive = &buffers[idx];
        match archive
            .get(pos..)
            .and_then(|r| r.iter().position(|&b| b != 0))
        {
            Some(n) => pos += n,
            None => return Ok(()),
        }
        let rest = &archive[pos..];
        if let Some(codec) = StreamCodec::sniff(rest) {
            // The kernel doesn't decompress again inside a decompressed
            // member, so neither does this.
            if idx != 0 {
                return Err(FoldError::Corrupt("cpio_compressed_inside_compressed"));
            }
            let (out, used) = decode_stream(codec, rest, *budget)?;
            *budget -= out.len();
            pos += used;
            buffers.push(out);
            let member = buffers.len() - 1;
            insert_chain(buffers, nodes, member, budget)?;
            continue;
        }
        let newc = rest.starts_with(CRC_MAGIC) || rest.starts_with(NEWC_MAGIC);
        if !newc || pos % 4 != 0 {
            return Err(FoldError::Corrupt("cpio_junk_between_archives"));
        }
        pos = insert_newc_archive(archive, idx, nodes, pos)?;
    }
}

/// Fold the newc/CRC archive at `start` of `archive` (buffer `idx`) into the
/// tree; returns the offset after its trailer.
fn insert_newc_archive(
    archive: &[u8],
    idx: usize,
    nodes: &mut Vec<Node>,
    start: usize,
) -> Result<usize> {
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

    // newc stores a hard-linked file's data once, with the last of its links,
    // and the others have none; every link gets that data.
    let mut linked: BTreeMap<(u32, u32, u32), (usize, usize)> = BTreeMap::new();
    for r in &records {
        if r.mode & S_IFMT == S_IFREG && r.nlink > 1 && r.data.1 > 0 {
            linked.insert(r.inode, r.data);
        }
    }
    for r in &records {
        let name = core::str::from_utf8(&archive[r.name.0..r.name.0 + r.name.1])
            .map_err(|_| FoldError::Unsupported("cpio_name_not_utf8"))?;
        let kind = file_kind(r.mode);
        let (off, len) = if kind == FileKind::Regular && r.nlink > 1 {
            linked.get(&r.inode).copied().unwrap_or(r.data)
        } else {
            r.data
        };
        insert(
            nodes,
            name,
            kind,
            r.mode & 0o7777,
            Span { buf: idx, off, len },
        );
    }
    Ok(pos)
}

/// Fold a single odc or binary archive into the tree (formats the kernel
/// doesn't take for an initramfs, so never chained).
fn insert_single(archive: &[u8], nodes: &mut Vec<Node>) {
    let base = archive.as_ptr() as usize;
    for entry in cpio_reader::iter_files(archive) {
        let file = entry.file();
        let off = file.as_ptr() as usize - base;
        insert(
            nodes,
            entry.name(),
            file_kind(entry.mode().bits()),
            entry.mode().bits() & 0o7777,
            Span {
                buf: 0,
                off,
                len: file.len(),
            },
        );
    }
}

/// A mounted cpio archive presented as a read-only tree.
///
/// The archive is read into memory at `open`, so the source `S` is not retained;
/// `PhantomData<S>` keeps the type monomorphic per source (the frontend pattern
/// the dispatcher expects) without storing one.
pub struct Cpio<S> {
    /// The input, then each decompressed member.
    buffers: Vec<Vec<u8>>,
    nodes: Vec<Node>,
    _src: PhantomData<S>,
}

impl<S> Cpio<S> {
    fn node(&self, id: NodeId) -> Result<&Node> {
        self.nodes.get(id as usize).ok_or(FoldError::NotFound)
    }

    fn bytes(&self, span: Span) -> &[u8] {
        &self.buffers[span.buf][span.off..span.off + span.len]
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

        let mut nodes = vec![Node::dir(0o755)]; // node 0 = root
        let newc = archive.starts_with(NEWC_MAGIC) || archive.starts_with(CRC_MAGIC);
        let mut buffers = vec![archive];
        if newc {
            // The decompressed members together get the same bound as one
            // whole-file read.
            let mut budget = checked_full_read_len(MAX_BOOT_FILE_BYTES)?;
            insert_chain(&mut buffers, &mut nodes, 0, &mut budget)?;
        } else {
            insert_single(&buffers[0], &mut nodes);
        }
        Ok(Cpio {
            buffers,
            nodes,
            _src: PhantomData,
        })
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
                n.data.len as u64
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
        let data = self.bytes(n.data);
        if off >= data.len() as u64 {
            return Ok(0);
        }
        let start = off as usize;
        let count = core::cmp::min(buf.len(), data.len() - start);
        buf[..count].copy_from_slice(&data[start..start + count]);
        Ok(count)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let n = self.node(node)?;
        if n.kind != FileKind::Symlink {
            return Ok(None);
        }
        Ok(Some(self.bytes(n.data).to_vec()))
    }
}
