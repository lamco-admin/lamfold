//! Integration test: a zisofs ISO (`xorriso ... -set_filter_r --zisofs`) —
//! verifies transparent paged-zlib decompression through the substrate codec.
//!
//! Fixture `fixtures/zisofs.iso`: `/big.txt`, 164000 bytes of a repeated line,
//! stored zisofs-compressed (spans several 32 KiB zisofs blocks).

#![cfg(feature = "zisofs")]

use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_iso::Iso9660;

const ZI: &[u8] = include_bytes!("fixtures/zisofs.iso");

fn payload() -> Vec<u8> {
    "lamfold zisofs compressible payload line\n"
        .repeat(4000)
        .into_bytes()
}

#[test]
fn zisofs_transparent_decompression() {
    let expected = payload();
    assert_eq!(expected.len(), 164000);

    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Iso9660::open(SliceSource::new(ZI), &mut cx).unwrap();
    let root = fs.root();

    let f = fs.lookup(root, "big.txt", &mut cx).unwrap().unwrap();

    // The reported size is the UNCOMPRESSED length (from the ZF entry), even
    // though the on-disk extent is smaller.
    let md = fs.metadata(f, &mut cx).unwrap();
    assert_eq!(md.size, 164000);

    // Full transparent decompression (multi-block).
    let mut buf = vec![0u8; md.size as usize];
    let n = fs.read_at(f, 0, &mut buf, &mut cx).unwrap();
    assert_eq!(n, 164000);
    assert_eq!(buf, expected);

    // Partial read straddling a zisofs block boundary (default 32 KiB).
    let mut mid = vec![0u8; 100];
    let mn = fs.read_at(f, 32700, &mut mid, &mut cx).unwrap();
    assert_eq!(mn, 100);
    assert_eq!(&mid[..], &expected[32700..32800]);

    // Read at the tail.
    let mut tail = vec![0u8; 64];
    let tn = fs.read_at(f, 163950, &mut tail, &mut cx).unwrap();
    assert_eq!(tn, 50);
    assert_eq!(&tail[..tn], &expected[163950..]);
}
