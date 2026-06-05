//! The shepherd, end to end: read EROFS file data through the frontend with a
//! `MerkleVerifier` in the `SubstrateCtx`. Genuine media verifies; a single
//! flipped byte on the medium is refused with `VerifyFailed` before any data is
//! surfaced. This is the integrity differentiator (`the lamfold design spec` §7) exercised
//! across the real L2↔L3 seam.

use lamfold::{
    BlockCache, FoldError, FoldFrontend, MerkleVerifier, SliceSource, SubstrateCtx,
    DEFAULT_BLOCK_LOG,
};
use lamfold_erofs::Erofs;

const IMG: &[u8] = include_bytes!("fixtures/erofs.img");

fn big_content() -> Vec<u8> {
    (0..300_000).map(|i| (i % 256) as u8).collect()
}

#[test]
fn shepherd_passes_genuine_and_refuses_tampered() {
    let genuine = big_content();
    // The trusted reference: leaf hashes + fs-verity measurement over the
    // genuine content. The measurement is the signable trust anchor.
    let verifier = MerkleVerifier::over(&genuine, DEFAULT_BLOCK_LOG, b"");
    assert_ne!(verifier.measurement(), [0u8; 32]);

    // Genuine image: every one of big.dat's 74 blocks verifies as it is read.
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &verifier,
    };
    let mut fs = Erofs::open(SliceSource::new(IMG), &mut cx).unwrap();
    let root = fs.root();
    let big = fs.lookup(root, "big.dat", &mut cx).unwrap().unwrap();
    let mut buf = vec![0u8; 300_000];
    assert_eq!(fs.read_at(big, 0, &mut buf, &mut cx).unwrap(), 300_000);
    assert_eq!(buf, genuine);

    // Tamper the medium: flip one byte inside big.dat's first full data block
    // (full blocks live at byte 4096 — raw_blkaddr 1).
    let mut tampered = IMG.to_vec();
    tampered[4096 + 100] ^= 0x01;

    let mut cache2 = BlockCache::new(0);
    let mut cx2 = SubstrateCtx {
        cache: &mut cache2,
        verifier: &verifier,
    };
    let mut fs2 = Erofs::open(SliceSource::new(&tampered), &mut cx2).unwrap();
    let root2 = fs2.root();
    let big2 = fs2.lookup(root2, "big.dat", &mut cx2).unwrap().unwrap();
    let mut buf2 = vec![0u8; 300_000];
    let err = fs2.read_at(big2, 0, &mut buf2, &mut cx2).unwrap_err();
    assert!(matches!(err, FoldError::VerifyFailed(_)), "got {err:?}");
}
