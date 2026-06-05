//! Integration test: walk a real plain-ISO9660 image (produced by `xorrisofs`)
//! end to end through the clean-room base reader.
//!
//! Fixture `fixtures/base.iso` contains:
//!   /README.TXT      35 bytes  "hello from lamfold iso base reader\n"
//!   /DIR1/           (directory)
//!   /DIR1/INNER.TXT  26 bytes  "nested file contents here\n"
//!   /DIR1/BIG.DAT    2500 bytes of 'A' (spans multiple 2048-byte sectors)

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_iso::Iso9660;

const ISO: &[u8] = include_bytes!("fixtures/base.iso");

fn ctx<'a>(cache: &'a mut BlockCache, verifier: &'a NoVerifier) -> SubstrateCtx<'a> {
    SubstrateCtx { cache, verifier }
}

#[test]
fn probe_accepts_iso9660() {
    let mut src = SliceSource::new(ISO);
    assert!(Iso9660::probe(&mut src).unwrap());
}

#[test]
fn probe_rejects_non_iso() {
    let junk = vec![0u8; 40 * 1024];
    let mut src = SliceSource::new(&junk);
    assert!(!Iso9660::probe(&mut src).unwrap());
}

#[test]
fn walks_tree_and_reads_files() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = ctx(&mut cache, &nov);

    let mut fs = Iso9660::open(SliceSource::new(ISO), &mut cx).unwrap();
    let root = fs.root();

    // root listing
    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"README.TXT".to_string()), "got {names:?}");
    assert!(names.contains(&"DIR1".to_string()), "got {names:?}");

    // small file read
    let readme = fs.lookup(root, "README.TXT", &mut cx).unwrap().unwrap();
    let md = fs.metadata(readme, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular);
    assert_eq!(md.size, 35);
    let mut buf = vec![0u8; md.size as usize];
    let n = fs.read_at(readme, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"hello from lamfold iso base reader\n");

    // nested directory + multi-sector file
    let dir1 = fs.lookup(root, "DIR1", &mut cx).unwrap().unwrap();
    assert_eq!(
        fs.metadata(dir1, &mut cx).unwrap().kind,
        FileKind::Directory
    );

    let inner = fs.lookup(dir1, "INNER.TXT", &mut cx).unwrap().unwrap();
    let mut ibuf = vec![0u8; 26];
    fs.read_at(inner, 0, &mut ibuf, &mut cx).unwrap();
    assert_eq!(&ibuf, b"nested file contents here\n");

    let big = fs.lookup(dir1, "BIG.DAT", &mut cx).unwrap().unwrap();
    let bmd = fs.metadata(big, &mut cx).unwrap();
    assert_eq!(bmd.size, 2500, "BIG.DAT spans multiple sectors");
    let mut bbuf = vec![0u8; 2500];
    let bn = fs.read_at(big, 0, &mut bbuf, &mut cx).unwrap();
    assert_eq!(bn, 2500);
    assert!(bbuf.iter().all(|&b| b == b'A'));

    // read at an offset across a sector boundary
    let mut mid = vec![0u8; 100];
    let mn = fs.read_at(big, 2040, &mut mid, &mut cx).unwrap();
    assert_eq!(mn, 100);
    assert!(mid.iter().all(|&b| b == b'A'));

    // missing entry
    assert!(fs.lookup(root, "NOPE.TXT", &mut cx).unwrap().is_none());
}
