//! UDF 2.01, from `fixtures/regenerate-udf201.sh`: mkudffs's default revision,
//! populated by the kernel, whose File Entries are all Extended File Entries
//! (tag 266). lamfold-udf 0.1.0 refused them as unsupported.

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_udf::Udf;

const IMG: &[u8] = include_bytes!("fixtures/udf201.img");

#[test]
fn extended_file_entries_read() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Udf::open(SliceSource::new(IMG), &mut cx).unwrap();
    let root = fs.root();
    let mut names: Vec<_> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, ["big.bin", "readme.txt", "sub"]);

    let readme = fs.lookup(root, "readme.txt", &mut cx).unwrap().unwrap();
    let mut buf = [0u8; 64];
    let n = fs.read_at(readme, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"udf 2.01 root file\n");

    let sub = fs.lookup(root, "sub", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(sub, &mut cx).unwrap().kind, FileKind::Directory);
    let inner = fs.lookup(sub, "inner.txt", &mut cx).unwrap().unwrap();
    let n = fs.read_at(inner, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"udf 2.01 nested file\n");

    let big = fs.lookup(root, "big.bin", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(big, &mut cx).unwrap().size, 70000);
    let mut all = vec![0u8; 70000];
    assert_eq!(fs.read_at(big, 0, &mut all, &mut cx).unwrap(), 70000);
    assert!(all
        .iter()
        .enumerate()
        .all(|(i, &b)| b == ((i * 11 + i / 257) & 0xff) as u8));
}
