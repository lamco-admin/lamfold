//! Integration test against a real `mkcramfs` image (cramfs-tools).
//!
//! Fixture `fixtures/cramfs.img`:
//!   /readme.txt    "romfs root file\n"  (16 B)   [shared tree]
//!   /sub/inner.txt "nested file\n"       (12 B)
//!   /big.dat       byte[i] = i % 256     (300000 B → 74 per-page zlib blocks)
//!   /link.txt   → readme.txt             (symlink, zlib-stored target)

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_cramfs::Cramfs;

const IMG: &[u8] = include_bytes!("fixtures/cramfs.img");

#[test]
fn probe_accepts_cramfs() {
    let mut src = SliceSource::new(IMG);
    assert!(Cramfs::probe(&mut src).unwrap());
}

#[test]
fn probe_rejects_non_cramfs() {
    let junk = vec![0u8; 4096];
    let mut src = SliceSource::new(&junk);
    assert!(!Cramfs::probe(&mut src).unwrap());
}

#[test]
fn walks_tree_reads_paged_files_and_symlink() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };

    let mut fs = Cramfs::open(SliceSource::new(IMG), &mut cx).unwrap();
    let root = fs.root();

    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"readme.txt".to_string()), "got {names:?}");
    assert!(names.contains(&"big.dat".to_string()), "got {names:?}");
    assert!(names.contains(&"sub".to_string()), "got {names:?}");
    assert!(names.contains(&"link.txt".to_string()), "got {names:?}");

    let readme = fs.lookup(root, "readme.txt", &mut cx).unwrap().unwrap();
    let md = fs.metadata(readme, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular);
    assert_eq!(md.size, 16);
    let mut buf = vec![0u8; 16];
    let n = fs.read_at(readme, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"romfs root file\n");

    let sub = fs.lookup(root, "sub", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(sub, &mut cx).unwrap().kind, FileKind::Directory);
    let inner = fs.lookup(sub, "inner.txt", &mut cx).unwrap().unwrap();
    let mut ibuf = vec![0u8; 12];
    fs.read_at(inner, 0, &mut ibuf, &mut cx).unwrap();
    assert_eq!(&ibuf, b"nested file\n");

    let link = fs.lookup(root, "link.txt", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(link, &mut cx).unwrap().kind, FileKind::Symlink);
    assert_eq!(fs.read_link(link, &mut cx).unwrap().unwrap(), b"readme.txt");

    // Multi-page zlib file (74 pages).
    let big = fs.lookup(root, "big.dat", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(big, &mut cx).unwrap().size, 300_000);
    let mut bbuf = vec![0u8; 300_000];
    let bn = fs.read_at(big, 0, &mut bbuf, &mut cx).unwrap();
    assert_eq!(bn, 300_000);
    assert!(bbuf.iter().enumerate().all(|(i, &b)| b == (i % 256) as u8));

    // partial read crossing a page boundary (4096)
    let mut mid = vec![0u8; 300];
    let mn = fs.read_at(big, 4_000, &mut mid, &mut cx).unwrap();
    assert_eq!(mn, 300);
    assert!(mid
        .iter()
        .enumerate()
        .all(|(i, &b)| b == ((4_000 + i) % 256) as u8));

    assert_eq!(fs.read_at(big, 300_000, &mut bbuf, &mut cx).unwrap(), 0);
    assert!(fs.lookup(root, "nope", &mut cx).unwrap().is_none());
}
