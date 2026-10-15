//! Multi-extent files, from `fixtures/regenerate-multiextent.sh`: one file
//! recorded as several directory records flagged Multi-Extent, as a file of
//! 4 GiB or more is. lamfold-iso 0.1.1 saw only the first record's extent.

use lamfold::{
    BlockCache, BlockSource, FileKind, FoldError, FoldFrontend, NoVerifier, NodeId, Result,
    SliceSource, SubstrateCtx,
};
use lamfold_iso::Iso9660;

const ISO: &[u8] = include_bytes!("fixtures/multiextent.iso");
const HUGE: &[u8] = include_bytes!("fixtures/multiextent-huge.iso");

fn big(i: usize) -> u8 {
    ((i * 13 + i / 509) & 0xff) as u8
}

/// The image, followed by zeros up to `len`: Room for extents of 4 GiB
/// without a 4 GiB fixture.
struct ZeroTail {
    img: &'static [u8],
    len: u64,
}

impl BlockSource for ZeroTail {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset + buf.len() as u64 > self.len {
            return Err(FoldError::Io("past the end"));
        }
        for (i, b) in buf.iter_mut().enumerate() {
            let at = offset + i as u64;
            *b = if at < self.img.len() as u64 {
                self.img[at as usize]
            } else {
                0
            };
        }
        Ok(())
    }
}

fn lookup<S: BlockSource>(fs: &mut Iso9660<S>, cx: &mut SubstrateCtx<'_>, name: &str) -> NodeId {
    let root = fs.root();
    fs.lookup(root, name, cx)
        .unwrap()
        .unwrap_or_else(|| panic!("{name}"))
}

#[test]
fn a_multi_extent_file_is_one_file() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Iso9660::open(SliceSource::new(ISO), &mut cx).unwrap();
    let root = fs.root();
    let names: Vec<_> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(
        names.iter().filter(|n| n.as_str() == "BIG.BIN").count(),
        1,
        "{names:?}"
    );
    let n = lookup(&mut fs, &mut cx, "BIG.BIN");
    let md = fs.metadata(n, &mut cx).unwrap();
    assert_eq!((md.kind, md.size), (FileKind::Regular, 10240));
    let mut all = vec![0u8; 10240];
    assert_eq!(fs.read_at(n, 0, &mut all, &mut cx).unwrap(), 10240);
    assert!(all.iter().enumerate().all(|(i, &b)| b == big(i)));
    // Windows across both extent boundaries.
    for at in [4090u64, 8190] {
        let mut w = [0u8; 12];
        assert_eq!(fs.read_at(n, at, &mut w, &mut cx).unwrap(), 12);
        assert!(
            w.iter()
                .enumerate()
                .all(|(i, &b)| b == big(at as usize + i)),
            "at {at}"
        );
    }
}

/// Extents adding up past 4 GiB: The size and offsets need 64 bits.
#[test]
fn a_file_over_4_gib_reads_its_last_extent() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let src = ZeroTail {
        img: HUGE,
        len: 16 << 30,
    };
    let mut fs = Iso9660::open(src, &mut cx).unwrap();
    let n = lookup(&mut fs, &mut cx, "BIG.BIN");
    let size = 2 * 4_294_965_248u64 + 20;
    assert_eq!(fs.metadata(n, &mut cx).unwrap().size, size);
    let mut tail = [0u8; 20];
    assert_eq!(fs.read_at(n, size - 20, &mut tail, &mut cx).unwrap(), 20);
    assert_eq!(&tail, b"multi-extent marker\n");
    let mut seam = [0xffu8; 8];
    assert_eq!(fs.read_at(n, size - 24, &mut seam, &mut cx).unwrap(), 8);
    assert_eq!(&seam, b"\0\0\0\0mult");
}
