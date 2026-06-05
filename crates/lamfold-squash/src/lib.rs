//! # lamfold-squash — the SquashFS frontend of the lamfold read-only media stack.
//!
//! Clean-room SquashFS 4.0 reader. SquashFS is the root filesystem inside almost
//! every Linux live ISO, so this frontend is the *recursion payoff*: mounted over
//! a file-inside-an-ISO (`FileBlockSource`) via `dispatch_fs_over_source`, it
//! reads the live root directly — the `lamoptical` + boot-from-ISO + read-the-root
//! convergence (`the lamfold design spec` §9).
//!
//! Reads over a lamfold [`lamfold::BlockSource`] and implements
//! [`lamfold::FoldFrontend`]; all decompression (gzip/xz/zstd/lz4/lzo) goes
//! through the shared substrate codec registry — the frontend describes only the
//! on-disk layout (superblock, compressed metadata blocks, inodes, directory
//! listings, data blocks, fragments).
//!
//! Scope: the read path for basic + extended directories/files, symlinks,
//! inline directory listings, full data blocks, and fragments. Deferred: xattrs,
//! the export/lookup table, the directory index (we linear-scan), and sparse
//! optimisation beyond zero-fill.
//!
//! Derived only from the public SquashFS format documentation (kernel
//! `Documentation/filesystems/squashfs` + the community spec); the GPL kernel
//! driver and `squashfs-tools` are fenced off — never read or copied.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod squash;

pub use squash::SquashFs;
