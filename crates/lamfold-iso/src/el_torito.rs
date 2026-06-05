//! El Torito boot-catalog parsing — locating the embedded **UEFI** boot image.
//!
//! This is what boot-from-ISO needs: a downloaded `.iso` exposes its UEFI boot
//! loader through an El Torito catalog, and LamBoot reads that image's byte
//! range (then chainloads it as the embedded FAT ESP, `the boot-from-ISO design notes`
//! path B). The catalog is a side-channel outside the filesystem surface — it
//! has no directory/inode analogue.
//!
//! Clean-roomed from the public El Torito v1.0 specification.

/// Platform id for UEFI El Torito entries (the 0xEF we want; 0x00 = x86 BIOS).
const PLATFORM_UEFI: u8 = 0xEF;
const BOOTABLE: u8 = 0x88;

/// The El Torito UEFI boot image as a byte-range on the optical source. The
/// range is `lba * 2048 .. + sectors * 512` (the LBA is a 2048-byte logical
/// block; the sector count is in 512-byte virtual sectors, per the spec).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UefiImage {
    pub lba: u32,
    pub sectors: u32,
}

/// Parse a boot catalog (one 2048-byte sector is plenty for the common case) and
/// return the first UEFI boot image, or `None` if there is no UEFI entry.
///
/// Handles both layouts real tools emit: a validation entry already on platform
/// 0xEF (the default/initial entry is then the UEFI image), and a BIOS default
/// entry followed by a 0xEF section header + section entries.
pub fn parse_catalog(cat: &[u8]) -> Option<UefiImage> {
    if cat.len() < 64 {
        return None;
    }
    // Validation Entry (offset 0): header_id 0x01, platform @1, key 0x55AA @30..32.
    if cat[0] != 0x01 || cat[30] != 0x55 || cat[31] != 0xAA {
        return None;
    }
    if cat[1] == PLATFORM_UEFI {
        // The Initial/Default Entry (offset 32) is the UEFI image.
        return parse_entry(&cat[32..64]);
    }
    // Otherwise walk Section Headers (offset 64) for a 0xEF platform.
    let mut off = 64;
    while off + 32 <= cat.len() {
        let hdr = &cat[off..off + 32];
        let indicator = hdr[0]; // 0x90 = more headers follow, 0x91 = final header
        if indicator != 0x90 && indicator != 0x91 {
            break;
        }
        let platform = hdr[1];
        let n_entries = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
        off += 32;
        if platform == PLATFORM_UEFI {
            for _ in 0..n_entries {
                if off + 32 > cat.len() {
                    break;
                }
                if let Some(img) = parse_entry(&cat[off..off + 32]) {
                    return Some(img);
                }
                off += 32;
            }
        } else {
            off = off.saturating_add(n_entries.saturating_mul(32));
        }
        if indicator == 0x91 {
            break;
        }
    }
    None
}

/// Parse a 32-byte Section/Default Entry: boot_indicator @0, sector_count (u16
/// LE) @6, load_rba (u32 LE) @8.
fn parse_entry(e: &[u8]) -> Option<UefiImage> {
    if e.len() < 12 || e[0] != BOOTABLE {
        return None;
    }
    let sectors = u32::from(u16::from_le_bytes([e[6], e[7]]));
    let lba = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
    Some(UefiImage { lba, sectors })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_validation_uefi_default_entry() {
        // The exact shape `xorrisofs -e ... -no-emul-boot` emits (see the et.iso
        // fixture): validation entry platform 0xEF, default entry = the image.
        let mut cat = [0u8; 96];
        cat[0] = 0x01; // header id
        cat[1] = 0xEF; // platform UEFI
        cat[30] = 0x55;
        cat[31] = 0xAA; // key
        cat[32] = 0x88; // bootable
        cat[38] = 0x04; // sector_count LE = 4
        cat[40] = 0x22; // load_rba LE = 0x22 = 34
        assert_eq!(
            parse_catalog(&cat),
            Some(UefiImage {
                lba: 34,
                sectors: 4
            })
        );
    }

    #[test]
    fn finds_uefi_section_after_bios_default() {
        let mut cat = [0u8; 128];
        cat[0] = 0x01;
        cat[1] = 0x00; // validation platform = BIOS
        cat[30] = 0x55;
        cat[31] = 0xAA;
        cat[32] = 0x88; // a BIOS default entry (ignored)
                        // Section header at offset 64: final (0x91), platform UEFI, 1 entry
        cat[64] = 0x91;
        cat[65] = 0xEF;
        cat[66] = 0x01;
        // Section entry at offset 96
        cat[96] = 0x88;
        cat[96 + 6] = 0x08; // 8 sectors
        cat[96 + 8] = 0x40; // lba 0x40 = 64
        assert_eq!(
            parse_catalog(&cat),
            Some(UefiImage {
                lba: 64,
                sectors: 8
            })
        );
    }

    #[test]
    fn no_uefi_entry_returns_none() {
        let mut cat = [0u8; 64];
        cat[0] = 0x01;
        cat[1] = 0x00;
        cat[30] = 0x55;
        cat[31] = 0xAA;
        assert_eq!(parse_catalog(&cat), None);
    }
}
