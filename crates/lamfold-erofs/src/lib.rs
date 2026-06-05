//! # lamfold-erofs — the EROFS frontend of the lamfold read-only media stack.
//!
//! Clean-room EROFS (Enhanced Read-Only File System) reader. EROFS is the
//! forward standard for immutable images — the metadata format **composefs**
//! builds on for content-addressed OS and OCI images — so this frontend is where
//! the lamfold stack meets the shepherd: read the metadata here, anchor trust in
//! [`lamfold`]'s native fs-verity Merkle (`the lamfold design spec` §7).
//!
//! Reads over a lamfold [`lamfold::BlockSource`] and implements
//! [`lamfold::FoldFrontend`]; with a [`lamfold::MerkleVerifier`] in the
//! [`lamfold::SubstrateCtx`] the read path verifies every data block against the
//! trusted leaf layer and refuses tampered media.
//!
//! Scope: the **uncompressed** read path — superblock, compact + extended
//! inodes, the `FLAT_PLAIN` and `FLAT_INLINE` data layouts, directory blocks,
//! and symlinks. Deferred: compressed clusters (lz4/lzma/zstd/deflate — the
//! second stage), chunk-based files, xattrs, and the shared-xattr area.
//!
//! Derived only from the public EROFS on-disk format — the kernel header
//! `fs/erofs/erofs_fs.h` is itself **SPDX MIT**, so it is referenced directly;
//! the GPL-2 `fs/erofs` *driver* is fenced off (never read or copied).

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod erofs;

pub use erofs::Erofs;
