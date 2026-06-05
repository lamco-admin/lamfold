//! A `FoldFrontend` tree built over the reused `cpio_reader` record parser.
//!
//! `cpio_reader` turns the archive bytes into an iterator of `(mode, name,
//! data)` records; this module reads the whole archive into memory, folds the
//! flat path list into a directory tree (creating intermediate directories that
//! a record implies), and stores each file/symlink as a `(offset, len)` view
//! into the archive so reads are zero-copy slices.

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
        let base = archive.as_ptr() as usize;
        for entry in cpio_reader::iter_files(&archive) {
            let ft = entry.mode().bits() & S_IFMT;
            let kind = match ft {
                S_IFDIR => FileKind::Directory,
                S_IFREG => FileKind::Regular,
                S_IFLNK => FileKind::Symlink,
                _ => FileKind::Other,
            };
            let file = entry.file();
            let off = file.as_ptr() as usize - base;
            me.insert(
                entry.name(),
                kind,
                entry.mode().bits() & 0o7777,
                (off, file.len()),
            );
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
