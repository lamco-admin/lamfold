//! Integration test against a real `mksquashfs -comp gzip` image.
//!
//! Fixture `fixtures/gzip.sqfs` (block size 128 KiB):
//!   /readme.txt    "squashfs root file\n"  (19 B  → packed in the shared fragment)
//!   /sub/inner.txt "squashfs nested\n"      (16 B  → packed in the shared fragment)
//!   /big.dat       byte[i] = i % 256        (300000 B → 3 full data blocks, no fragment)
//!
//! Small files exercise the fragment (tail-packing) read path; `big.dat`
//! exercises multi-block data reassembly. Both go through the substrate gzip
//! codec for every compressed metadata + data block.

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_squash::SquashFs;

const IMG: &[u8] = include_bytes!("fixtures/gzip.sqfs");

fn ctx() -> (BlockCache, NoVerifier) {
    (BlockCache::new(0), NoVerifier)
}

#[test]
fn probe_accepts_squashfs() {
    let mut src = SliceSource::new(IMG);
    assert!(SquashFs::probe(&mut src).unwrap());
}

#[test]
fn probe_rejects_non_squashfs() {
    let junk = vec![0u8; 8192];
    let mut src = SliceSource::new(&junk);
    assert!(!SquashFs::probe(&mut src).unwrap());
}

#[test]
fn walks_tree_and_reads_fragment_and_block_files() {
    let (mut cache, nov) = ctx();
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };

    let mut fs = SquashFs::open(SliceSource::new(IMG), &mut cx).unwrap();
    let root = fs.root();

    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"readme.txt".to_string()), "got {names:?}");
    assert!(names.contains(&"sub".to_string()), "got {names:?}");
    assert!(names.contains(&"big.dat".to_string()), "got {names:?}");

    // Fragment-packed small file in the root.
    let readme = fs.lookup(root, "readme.txt", &mut cx).unwrap().unwrap();
    let md = fs.metadata(readme, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular);
    assert_eq!(md.size, 19);
    let mut buf = vec![0u8; 19];
    let n = fs.read_at(readme, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"squashfs root file\n");

    // Nested directory + fragment-packed file.
    let sub = fs.lookup(root, "sub", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(sub, &mut cx).unwrap().kind, FileKind::Directory);
    let inner = fs.lookup(sub, "inner.txt", &mut cx).unwrap().unwrap();
    let mut ibuf = vec![0u8; 16];
    fs.read_at(inner, 0, &mut ibuf, &mut cx).unwrap();
    assert_eq!(&ibuf, b"squashfs nested\n");

    // Multi-block data file (3 × 128 KiB blocks, gzip-compressed).
    let big = fs.lookup(root, "big.dat", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(big, &mut cx).unwrap().size, 300_000);
    let mut bbuf = vec![0u8; 300_000];
    let bn = fs.read_at(big, 0, &mut bbuf, &mut cx).unwrap();
    assert_eq!(bn, 300_000);
    assert!(bbuf.iter().enumerate().all(|(i, &b)| b == (i % 256) as u8));

    // Partial read straddling the 2nd→3rd data block boundary (262144).
    let mut mid = vec![0u8; 400];
    let mn = fs.read_at(big, 261_900, &mut mid, &mut cx).unwrap();
    assert_eq!(mn, 400);
    assert!(mid
        .iter()
        .enumerate()
        .all(|(i, &b)| b == ((261_900 + i) % 256) as u8));

    // EOF read returns 0.
    assert_eq!(fs.read_at(big, 300_000, &mut bbuf, &mut cx).unwrap(), 0);
    assert!(fs.lookup(root, "nope", &mut cx).unwrap().is_none());
}
