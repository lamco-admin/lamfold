//! Read-capacity hardening — the chokepoints every allocation-from-a-size-field
//! goes through, so a hostile on-disk size can never drive an unbounded up-front
//! allocation (a denial-of-boot) and a 32-bit build fails loudly instead of
//! truncating an `as usize` cast.
//!
//! This is the lamfold port of LamBoot's `read_limit_pure` / the FSB-01..07
//! read contract (`the fs-backend design notes`). Pure + host-tested.

use crate::error::{FoldError, Result};

/// Largest single file lamfold will allocate for a full read. 256 MiB comfortably
/// covers a kernel + initrd while bounding a hostile inode size.
pub const MAX_BOOT_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Largest single decompressed block lamfold will produce. Filesystem block /
/// cluster sizes are small (squashfs ≤ 1 MiB, erofs clusters smaller); 16 MiB is
/// a generous ceiling that still defuses a decompression bomb.
pub const MAX_DECOMPRESSED_BLOCK_BYTES: u64 = 16 * 1024 * 1024;

// A single block cap must never exceed the whole-file cap — enforced at compile
// time so the invariant can't silently regress.
const _: () = assert!(MAX_DECOMPRESSED_BLOCK_BYTES < MAX_BOOT_FILE_BYTES);

/// Validate a metadata-reported full-file size before `vec![0u8; n]`. Rejects
/// anything over [`MAX_BOOT_FILE_BYTES`] or that does not fit `usize`.
pub fn checked_full_read_len(size: u64) -> Result<usize> {
    checked_len(size, MAX_BOOT_FILE_BYTES)
}

/// Validate a declared decompressed-block size before allocating its output
/// buffer (the decompression-bomb guard).
pub fn checked_block_len(size: u64) -> Result<usize> {
    checked_len(size, MAX_DECOMPRESSED_BLOCK_BYTES)
}

fn checked_len(size: u64, max: u64) -> Result<usize> {
    if size > max {
        return Err(FoldError::FileTooLarge { size, max });
    }
    usize::try_from(size).map_err(|_| FoldError::FileTooLarge { size, max })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_within_cap() {
        assert_eq!(checked_full_read_len(0).unwrap(), 0);
        assert_eq!(checked_full_read_len(1024).unwrap(), 1024);
        assert_eq!(
            checked_full_read_len(MAX_BOOT_FILE_BYTES).unwrap() as u64,
            MAX_BOOT_FILE_BYTES
        );
    }

    #[test]
    fn rejects_over_cap() {
        let e = checked_full_read_len(MAX_BOOT_FILE_BYTES + 1).unwrap_err();
        assert!(matches!(e, FoldError::FileTooLarge { .. }));
        assert_eq!(e.as_token(), "file_too_large");
    }

    #[test]
    fn block_cap_is_independent_and_smaller() {
        // (the cap ordering itself is asserted at compile time in this module)
        assert!(checked_block_len(MAX_DECOMPRESSED_BLOCK_BYTES).is_ok());
        assert!(checked_block_len(MAX_DECOMPRESSED_BLOCK_BYTES + 1).is_err());
    }

    #[test]
    fn u64_that_overflows_usize_is_rejected_not_truncated() {
        // On a 32-bit host this is the load-bearing case; on 64-bit the cap
        // catches it first, which is also correct.
        let huge = u64::MAX;
        assert!(matches!(
            checked_full_read_len(huge),
            Err(FoldError::FileTooLarge { .. })
        ));
    }
}
