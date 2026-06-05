//! Integration test: a real Rock Ridge ISO (`xorrisofs -R`) — verifies the
//! POSIX layer (real lowercase/long names via NM, symlinks via SL).
//!
//! Fixture `fixtures/rr.iso`:
//!   /readme.txt        "rock ridge gives real lowercase names\n"
//!   /link.txt   ->     readme.txt   (symlink)
//!   /subdir/inner.txt  "inner rr file\n"

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_iso::Iso9660;

const RR: &[u8] = include_bytes!("fixtures/rr.iso");

#[test]
fn rock_ridge_real_names_and_symlinks() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };

    let mut fs = Iso9660::open(SliceSource::new(RR), &mut cx).unwrap();
    let root = fs.root();

    // Real Rock Ridge names (lowercase) — NOT the mangled 8.3 base names.
    let names: Vec<String> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"readme.txt".to_string()), "got {names:?}");
    assert!(names.contains(&"link.txt".to_string()), "got {names:?}");
    assert!(names.contains(&"subdir".to_string()), "got {names:?}");
    // the base name READExxx.TXT;1 must NOT leak through
    assert!(
        !names.iter().any(|n| n.contains(';') || n.contains("READ")),
        "base 8.3 name leaked: {names:?}"
    );

    // readme.txt: real name resolves + content reads
    let readme = fs.lookup(root, "readme.txt", &mut cx).unwrap().unwrap();
    let md = fs.metadata(readme, &mut cx).unwrap();
    assert_eq!(md.kind, FileKind::Regular);
    let mut buf = vec![0u8; md.size as usize];
    let n = fs.read_at(readme, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(&buf[..n], b"rock ridge gives real lowercase names\n");

    // link.txt is a symlink whose target is readme.txt
    let link = fs.lookup(root, "link.txt", &mut cx).unwrap().unwrap();
    assert_eq!(fs.metadata(link, &mut cx).unwrap().kind, FileKind::Symlink);
    let target = fs.read_link(link, &mut cx).unwrap().unwrap();
    assert_eq!(target, b"readme.txt");

    // a non-symlink reports no link target
    assert!(fs.read_link(readme, &mut cx).unwrap().is_none());

    // nested real name
    let subdir = fs.lookup(root, "subdir", &mut cx).unwrap().unwrap();
    let inner = fs.lookup(subdir, "inner.txt", &mut cx).unwrap().unwrap();
    let mut ibuf = vec![0u8; fs.metadata(inner, &mut cx).unwrap().size as usize];
    fs.read_at(inner, 0, &mut ibuf, &mut cx).unwrap();
    assert_eq!(&ibuf, b"inner rr file\n");
}
