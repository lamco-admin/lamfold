//! Chained initramfs archives, built by `fixtures/regenerate.sh` with GNU cpio:
//! an early (microcode) archive followed by the main one, plain or compressed.
//! lamfold-cpio 0.1.0 stopped at the first archive's trailer and read hard
//! links as empty files; 0.1.1 refused every compressed main archive.

use lamfold::{
    BlockCache, FileKind, FoldError, FoldFrontend, NoVerifier, NodeId, SliceSource, SubstrateCtx,
};
use lamfold_cpio::Cpio;

const PLAIN: &[u8] = include_bytes!("fixtures/chain_plain.cpio");
const COMPRESSED: &[(&str, &[u8])] = &[
    ("gzip", include_bytes!("fixtures/chain_gzip.cpio")),
    ("zstd", include_bytes!("fixtures/chain_zstd.cpio")),
    ("xz", include_bytes!("fixtures/chain_xz.cpio")),
    ("lzma", include_bytes!("fixtures/chain_lzma.cpio")),
    ("lz4", include_bytes!("fixtures/chain_lz4.cpio")),
];
const BZIP2: &[u8] = include_bytes!("fixtures/chain_bzip2.cpio");
const NESTED: &[u8] = include_bytes!("fixtures/nested_gzip.cpio");

fn with_fs<T>(
    img: &'static [u8],
    f: impl FnOnce(&mut Cpio<SliceSource<'static>>, &mut SubstrateCtx<'_>) -> T,
) -> T {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Cpio::open(SliceSource::new(img), &mut cx).unwrap();
    f(&mut fs, &mut cx)
}

fn node(fs: &mut Cpio<SliceSource<'static>>, cx: &mut SubstrateCtx<'_>, path: &str) -> NodeId {
    let mut cur = fs.root();
    for comp in path.split('/') {
        cur = fs
            .lookup(cur, comp, cx)
            .unwrap()
            .unwrap_or_else(|| panic!("{path}: {comp} missing"));
    }
    cur
}

fn read(fs: &mut Cpio<SliceSource<'static>>, cx: &mut SubstrateCtx<'_>, path: &str) -> Vec<u8> {
    let n = node(fs, cx, path);
    let size = fs.metadata(n, cx).unwrap().size as usize;
    let mut buf = vec![0u8; size];
    assert_eq!(fs.read_at(n, 0, &mut buf, cx).unwrap(), size, "{path}");
    buf
}

/// Both archives' files are in the tree, the main one in the CRC format.
#[test]
fn every_archive_in_the_chain_is_read() {
    with_fs(PLAIN, |fs, cx| {
        assert_eq!(
            read(fs, cx, "kernel/x86/microcode/GenuineIntel.bin"),
            b"microcode blob for the fixture\n"
        );
        assert_eq!(read(fs, cx, "init"), b"#!/bin/sh\necho init\n");
        assert_eq!(
            read(fs, cx, "etc/os-release"),
            b"NAME=\"lamfold-cpio fixture\"\n"
        );
        let sh = node(fs, cx, "bin/sh");
        assert_eq!(fs.read_link(sh, cx).unwrap().unwrap(), b"busybox");
        let run = node(fs, cx, "run");
        assert_eq!(fs.metadata(run, cx).unwrap().kind, FileKind::Directory);
    });
}

/// newc stores a hard-linked file's data once, on its last link; every link
/// reads it.
#[test]
fn every_hard_link_reads_the_data() {
    with_fs(PLAIN, |fs, cx| {
        for name in ["lib/a.so", "lib/b.so", "lib/c.so"] {
            assert_eq!(read(fs, cx, name), b"one file, three names\n", "{name}");
        }
    });
}

/// A compressed main archive is decoded, and the uncompressed archive after it
/// is found where the compressed member ends.
#[test]
fn compressed_main_archives_read() {
    for &(codec, img) in COMPRESSED {
        with_fs(img, |fs, cx| {
            assert_eq!(
                read(fs, cx, "kernel/x86/microcode/GenuineIntel.bin"),
                b"microcode blob for the fixture\n",
                "{codec}"
            );
            assert_eq!(read(fs, cx, "init"), b"#!/bin/sh\necho init\n", "{codec}");
            for name in ["lib/a.so", "lib/b.so", "lib/c.so"] {
                assert_eq!(
                    read(fs, cx, name),
                    b"one file, three names\n",
                    "{codec} {name}"
                );
            }
            let sh = node(fs, cx, "bin/sh");
            assert_eq!(
                fs.read_link(sh, cx).unwrap().unwrap(),
                b"busybox",
                "{codec}"
            );
            assert_eq!(
                read(fs, cx, "etc/late.conf"),
                b"written by the late archive\n",
                "{codec}"
            );
        });
    }
}

fn open_err(img: &'static [u8]) -> FoldError {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    match Cpio::open(SliceSource::new(img), &mut cx) {
        Err(e) => e,
        Ok(_) => panic!("opened"),
    }
}

/// bzip2 is recognised but not decoded, and refused rather than read empty.
#[test]
fn a_bzip2_archive_is_refused() {
    assert!(matches!(
        open_err(BZIP2),
        FoldError::Unsupported("stream_bzip2")
    ));
}

/// The kernel doesn't decompress inside a decompressed member.
#[test]
fn compression_inside_compression_is_refused() {
    assert!(matches!(
        open_err(NESTED),
        FoldError::Corrupt("cpio_compressed_inside_compressed")
    ));
}

/// A CRC-format member whose checksum doesn't match its data is corrupt.
#[test]
fn a_bad_crc_is_corrupt() {
    let mut img = PLAIN.to_vec();
    // Flip a byte of /init's data, inside the CRC-format main archive.
    let at = img
        .windows(10)
        .position(|w| w == b"echo init\n")
        .expect("init data");
    img[at] ^= 0x20;
    let img: &'static [u8] = Box::leak(img.into_boxed_slice());
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    assert!(matches!(
        Cpio::open(SliceSource::new(img), &mut cx),
        Err(FoldError::Corrupt(_))
    ));
}
