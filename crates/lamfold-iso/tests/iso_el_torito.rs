//! Integration test: a real UEFI El Torito ISO (`xorrisofs -e ... -no-emul-boot`)
//! — verifies the boot-catalog side-channel that boot-from-ISO relies on.
//!
//! Fixture `fixtures/et.iso`: a UEFI El Torito entry pointing at a 2048-byte
//! "boot image" (`efiboot.img`, content "LAMFOLD-EFI-BOOT-IMAGE…") at LBA 34,
//! load-size 4 (×512 = 2048 bytes).

use lamfold::{BlockCache, BlockSource, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use lamfold_iso::{Iso9660, UefiImage};

const ET: &[u8] = include_bytes!("fixtures/et.iso");

#[test]
fn locates_uefi_boot_image_and_reads_it() {
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };

    let mut fs = Iso9660::open(SliceSource::new(ET), &mut cx).unwrap();

    // The side-channel resolves the UEFI image's byte range.
    let img = fs
        .el_torito_uefi_image()
        .unwrap()
        .expect("UEFI El Torito entry");
    assert_eq!(
        img,
        UefiImage {
            lba: 34,
            sectors: 4
        }
    );

    // Read the image at its byte range (lba*2048 .. + sectors*512) and confirm
    // it is the embedded boot image — i.e. the LBA actually points at it.
    let mut src = SliceSource::new(ET);
    let byte_off = u64::from(img.lba) * 2048;
    let len = (img.sectors * 512) as usize;
    let mut buf = vec![0u8; len];
    src.read_at(byte_off, &mut buf).unwrap();
    assert_eq!(len, 2048);
    assert!(
        buf.starts_with(b"LAMFOLD-EFI-BOOT-IMAGE"),
        "El Torito LBA did not point at the boot image: {:?}",
        &buf[..24]
    );
}

#[test]
fn plain_iso_has_no_uefi_boot_image() {
    // The base (non-bootable) fixture must report no El Torito entry.
    const BASE: &[u8] = include_bytes!("fixtures/base.iso");
    let mut cache = BlockCache::new(0);
    let nov = NoVerifier;
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &nov,
    };
    let mut fs = Iso9660::open(SliceSource::new(BASE), &mut cx).unwrap();
    assert!(fs.el_torito_uefi_image().unwrap().is_none());
}
