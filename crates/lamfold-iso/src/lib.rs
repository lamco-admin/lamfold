//! # lamfold-iso — the optical frontend of the lamfold read-only media stack.
//!
//! Clean-room ISO9660 / ECMA-119 reader (the base), with Rock Ridge, Joliet,
//! El Torito, and zisofs layered on separately. Reads over a lamfold
//! [`lamfold::BlockSource`] and implements [`lamfold::FoldFrontend`], so it
//! composes through LamBoot's `dispatch_fs_over_source` — including recursively,
//! over a file inside another volume. Supersedes the earlier `lamoptical`
//! scaffold. See `the lamfold design spec` §4.
//!
//! Derived only from public specifications (ECMA-119, IEEE P1281/P1282, the El
//! Torito spec) and permissive references (see `NOTICE`); no GPL implementation
//! (libcdio, Linux `fs/isofs`) is read or copied.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod iso9660;
mod rock_ridge;

pub use iso9660::Iso9660;
