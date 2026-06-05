//! # lamfold-udf — the UDF frontend of the lamfold read-only media stack.
//!
//! Clean-room UDF (ECMA-167 / OSTA UDF **1.02**) reader: the descriptor chain
//! AVDP → VDS (Partition + Logical Volume) → File Set → root ICB, then File
//! Entries and File Identifier Descriptors. Reads over a lamfold
//! [`lamfold::BlockSource`] and implements [`lamfold::FoldFrontend`], so it
//! composes through `dispatch_fs_over_source` like every other frontend, and
//! co-resides with `lamfold-iso` on UDF-Bridge optical media.
//!
//! Scope: UDF 1.02 with a single physical (Type-1) partition map, File
//! Entry (tag 261) inodes, and inline / short_ad / long_ad data. Deferred (the
//! 2.50+ surface): Extended File Entry (266), metadata/sparable/virtual (VAT)
//! partition maps, extended_ad, and named streams — see `the lamfold design spec` §4.
//!
//! Derived only from the free ECMA-167 + OSTA UDF specifications (see `NOTICE`);
//! the GPL Linux `fs/udf` and `udftools` are fenced off — never read or copied.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod udf;

pub use udf::Udf;
