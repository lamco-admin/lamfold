//! Rock Ridge and El Torito cases from xorriso images that lamfold-iso 0.1.0 read
//! wrong, built by `fixtures/regenerate-xorriso.sh`.

use std::collections::BTreeSet;

use lamfold::{BlockCache, FileKind, FoldFrontend, NoVerifier, NodeId, SliceSource, SubstrateCtx};
use lamfold_iso::{Iso9660, UefiImage};

const RR: &[u8] = include_bytes!("fixtures/rr_links.iso");
const LONG_LINK: &str = include_str!("fixtures/rr_links.longlink");
const LONG_NAME: &str = include_str!("fixtures/rr_links.longname");
const ET: &[u8] = include_bytes!("fixtures/et_appended.iso");

fn with_fs<T>(
    img: &[u8],
    f: impl FnOnce(&mut Iso9660<SliceSource<'_>>, &mut SubstrateCtx<'_>) -> T,
) -> T {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Iso9660::open(SliceSource::new(img), &mut cx).unwrap();
    f(&mut fs, &mut cx)
}

fn node(fs: &mut Iso9660<SliceSource<'_>>, cx: &mut SubstrateCtx<'_>, name: &str) -> NodeId {
    let root = fs.root();
    fs.lookup(root, name, cx)
        .unwrap_or_else(|e| panic!("lookup {name}: {e:?}"))
        .unwrap_or_else(|| panic!("{name} missing"))
}

/// xorriso gives every empty file and every symlink the same extent LBA. 0.1.0
/// keyed nodes by that LBA, so they all became the first one.
#[test]
fn every_symlink_and_empty_file_is_its_own_node() {
    with_fs(RR, |fs, cx| {
        let links = [
            ("link-rel", "a.txt".to_string()),
            ("link-abs", "/usr/bin/bash".to_string()),
            ("link-dir", "sub".to_string()),
            ("link-long", LONG_LINK.trim_end().to_string()),
        ];
        let mut nodes = BTreeSet::new();
        for (name, target) in &links {
            let n = node(fs, cx, name);
            assert_eq!(
                fs.metadata(n, cx).unwrap().kind,
                FileKind::Symlink,
                "{name}"
            );
            let got = fs
                .read_link(n, cx)
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no target"));
            assert_eq!(String::from_utf8(got).unwrap(), *target, "{name}");
            nodes.insert(n);
        }
        for name in ["empty1", "empty2", "empty3"] {
            let n = node(fs, cx, name);
            let md = fs.metadata(n, cx).unwrap();
            assert_eq!((md.kind, md.size), (FileKind::Regular, 0), "{name}");
            nodes.insert(n);
        }
        assert_eq!(
            nodes.len(),
            7,
            "each link and empty file is a distinct node"
        );
    });
}

/// A 250-character name and a 300-byte link target don't fit in their
/// directory records; the rest is in a continuation area. 0.1.0 ignored it
/// and returned the first 93 bytes of the name.
#[test]
fn long_names_and_targets_follow_continuation_areas() {
    with_fs(RR, |fs, cx| {
        let root = fs.root();
        let names: Vec<String> = fs
            .read_dir(root, cx)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        let long = LONG_NAME.trim_end();
        assert_eq!(long.len(), 250);
        assert!(
            names.iter().any(|n| n == long),
            "250-character name missing from {names:?}"
        );
        let n = node(fs, cx, long);
        let mut buf = [0u8; 32];
        let got = fs.read_at(n, 0, &mut buf, cx).unwrap();
        assert_eq!(&buf[..got], b"long name\n");
    });
}

/// The UEFI boot image when the catalog's sector count is 0, as on Arch's
/// ISO (its EFI image is larger than the 16-bit count holds): The size comes
/// from the image's FAT boot sector. 0.1.0 returned a zero-length image.
#[test]
fn uefi_image_size_comes_from_fat_when_the_count_is_zero() {
    let mut img = ET.to_vec();
    let catalog = 33 * 2048;
    assert_eq!(
        u16::from_le_bytes([img[catalog + 38], img[catalog + 39]]),
        128
    );
    img[catalog + 38] = 0;
    img[catalog + 39] = 0;
    with_fs(&img, |fs, _| {
        assert_eq!(
            fs.el_torito_uefi_image().unwrap(),
            Some(UefiImage {
                lba: 35,
                sectors: 128
            })
        );
    });
}
