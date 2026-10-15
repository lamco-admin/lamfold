//! Per-block reads, from `fixtures/regenerate-large.sh` (mksquashfs, zstd):
//! lamfold-squash 0.1.0 decoded the whole file on every `read_at` and so
//! refused any file over the 512 MiB whole-file cap.

use lamfold::{BlockCache, FoldFrontend, NoVerifier, NodeId, SliceSource, SubstrateCtx};
use lamfold_squash::SquashFs;

const IMG: &[u8] = include_bytes!("fixtures/large.sqfs");
const HUGE: u64 = 600 * 1024 * 1024;

fn multi(i: usize) -> u8 {
    ((i * 7 + i / 251) & 0xFF) as u8
}

fn with_fs(f: impl FnOnce(&mut SquashFs<SliceSource<'static>>, &mut SubstrateCtx<'_>)) {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = SquashFs::open(SliceSource::new(IMG), &mut cx).unwrap();
    f(&mut fs, &mut cx);
}

fn node(fs: &mut SquashFs<SliceSource<'static>>, cx: &mut SubstrateCtx<'_>, name: &str) -> NodeId {
    let root = fs.root();
    fs.lookup(root, name, cx).unwrap().unwrap()
}

/// The end of a 600 MiB file reads without the whole file in memory.
#[test]
fn a_file_over_the_whole_file_cap_reads() {
    with_fs(|fs, cx| {
        let n = node(fs, cx, "huge.bin");
        assert_eq!(fs.metadata(n, cx).unwrap().size, HUGE);
        let mut tail = [0u8; 64];
        assert_eq!(fs.read_at(n, HUGE - 64, &mut tail, cx).unwrap(), 64);
        assert!(tail.starts_with(b"end of a 600 MiB file"));
        let mut middle = [0xffu8; 4096];
        assert_eq!(
            fs.read_at(n, 300 * 1024 * 1024, &mut middle, cx).unwrap(),
            4096
        );
        assert!(
            middle.iter().all(|&b| b == 0),
            "sparse blocks read as zeros"
        );
    });
}

/// Reads of odd sizes cross block boundaries and the fragment tail.
#[test]
fn windows_cross_blocks_and_the_tail() {
    with_fs(|fs, cx| {
        let n = node(fs, cx, "multi.bin");
        let mut got = Vec::new();
        let mut off = 0u64;
        loop {
            let mut buf = [0u8; 7001];
            let k = fs.read_at(n, off, &mut buf, cx).unwrap();
            if k == 0 {
                break;
            }
            got.extend_from_slice(&buf[..k]);
            off += k as u64;
        }
        assert_eq!(got.len(), 300000);
        assert!(got.iter().enumerate().all(|(i, &b)| b == multi(i)));
        let mut edge = [0u8; 20];
        assert_eq!(fs.read_at(n, 131072 - 10, &mut edge, cx).unwrap(), 20);
        assert!(edge
            .iter()
            .enumerate()
            .all(|(i, &b)| b == multi(131062 + i)));
    });
}
