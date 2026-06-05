//! Integration test: a Joliet ISO (`xorrisofs -J`, no Rock Ridge) — verifies the
//! UCS-2 (UTF-16BE) name tree is selected and decoded.
//!
//! Fixture `fixtures/joliet.iso`:
//!   /Joliet Long Name.txt           "joliet unicode long name file\n"
//!   /Folder With Spaces/Inner File.txt  "inner joliet\n"

use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_iso::Iso9660;

const JOL: &[u8] = include_bytes!("fixtures/joliet.iso");

#[test]
fn joliet_long_unicode_names() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };

    let mut fs = Iso9660::open(SliceSource::new(JOL), &mut cx).unwrap();
    let root = fs.root();

    // Long names with spaces come through verbatim (not mangled 8.3).
    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(
        names.contains(&"Joliet Long Name.txt".to_string()),
        "got {names:?}"
    );
    assert!(
        names.contains(&"Folder With Spaces".to_string()),
        "got {names:?}"
    );

    // Resolve a long name + read content.
    let f = fs
        .lookup(root, "Joliet Long Name.txt", &mut cx)
        .unwrap()
        .unwrap();
    let mut buf = vec![0u8; fs.metadata(f, &mut cx).unwrap().size as usize];
    fs.read_at(f, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf, b"joliet unicode long name file\n");

    // Descend a spaced directory.
    let dir = fs
        .lookup(root, "Folder With Spaces", &mut cx)
        .unwrap()
        .unwrap();
    let inner = fs.lookup(dir, "Inner File.txt", &mut cx).unwrap().unwrap();
    let mut ibuf = vec![0u8; fs.metadata(inner, &mut cx).unwrap().size as usize];
    fs.read_at(inner, 0, &mut ibuf, &mut cx).unwrap();
    assert_eq!(&ibuf, b"inner joliet\n");
}
