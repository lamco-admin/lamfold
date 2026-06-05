//! # lamfold-cpio — the initramfs/cpio frontend of the lamfold stack.
//!
//! cpio is the archive format of the Linux initramfs/initrd — a flat stream of
//! `(header, path, data)` records, not a block filesystem. The on-disk parsing
//! is trivial and a permissive `no_std` reader already exists, so — per the SDS
//! "reuse where licensing does not hinder" rule — this frontend **reuses**
//! [`cpio_reader`] (MIT OR Apache-2.0) for record parsing and adds only what
//! lamfold needs: build the flat path list into a directory tree and present it
//! through [`lamfold::FoldFrontend`], so an initramfs reads through the same
//! `dispatch_fs_over_source` path as a real filesystem.
//!
//! Scope: newc / CRC / portable-ASCII / old-binary cpio (all handled by
//! `cpio_reader`), regular files, directories (explicit or implied by a nested
//! path), and symlinks (`read_link`). The whole archive is read into memory
//! (read-capped) — appropriate for an initramfs, which is resident anyway.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod cpio;

pub use cpio::Cpio;
