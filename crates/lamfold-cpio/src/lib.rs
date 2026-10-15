//! # lamfold-cpio: the initramfs and cpio frontend of the lamfold stack.
//!
//! cpio is the archive format of the Linux initramfs: a flat stream of
//! `(header, path, data)` records, not a block filesystem. An initramfs is
//! often several archives back to back (an uncompressed early archive with CPU
//! microcode, then the main one), and this frontend reads every newc and CRC
//! archive in such a chain, as the kernel unpacks them, into one directory
//! tree presented through [`lamfold::FoldFrontend`], so an initramfs reads
//! through the same `dispatch_fs_over_source` path as a real filesystem.
//!
//! Scope: chained newc and CRC archives (parsed here, since the parser has to
//! know where each archive ends), single odc and old binary archives (through
//! the reused [`cpio_reader`], MIT OR Apache-2.0), regular files, hard links,
//! directories (explicit or implied by a nested path), and symlinks
//! (`read_link`). A compressed archive in the chain is refused as
//! `Unsupported("cpio_compressed_member")`. The whole input is read into
//! memory (read-capped), which suits an initramfs.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod cpio;

pub use cpio::Cpio;
