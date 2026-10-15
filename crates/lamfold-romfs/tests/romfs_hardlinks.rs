//! Hard links, from `fixtures/regenerate-hardlinks.sh` (genromfs): b.txt and
//! sub/c.txt are type-0 headers naming a.txt's header. lamfold-romfs 0.1.0
//! listed them as `Other` with no data.

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_romfs::Romfs;

const IMG: &[u8] = include_bytes!("fixtures/hardlinks.img");

#[test]
fn every_hard_link_reads_its_target() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Romfs::open(SliceSource::new(IMG), &mut cx).unwrap();
    let root = fs.root();
    let sub = fs.lookup(root, "sub", &mut cx).unwrap().unwrap();
    let a = fs.lookup(root, "a.txt", &mut cx).unwrap().unwrap();
    for (dir, name) in [(root, "a.txt"), (root, "b.txt"), (sub, "c.txt")] {
        let node = fs.lookup(dir, name, &mut cx).unwrap().unwrap();
        assert_eq!(node, a, "{name} is the same file as a.txt");
        let md = fs.metadata(node, &mut cx).unwrap();
        assert_eq!((md.kind, md.size), (FileKind::Regular, 22), "{name}");
        let mut buf = [0u8; 22];
        assert_eq!(fs.read_at(node, 0, &mut buf, &mut cx).unwrap(), 22);
        assert_eq!(&buf, b"shared by three names\n", "{name}");
    }
    let kinds: Vec<_> = fs
        .read_dir(root, &mut cx)
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.kind))
        .collect();
    assert!(
        kinds.contains(&("b.txt".into(), FileKind::Regular)),
        "{kinds:?}"
    );
    let link = fs.lookup(root, "link", &mut cx).unwrap().unwrap();
    assert_eq!(fs.read_link(link, &mut cx).unwrap().unwrap(), b"a.txt");
}
