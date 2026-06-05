//! # lamfold — read-only media filesystem stack (substrate core)
//!
//! `lamfold` is an **immutable-media filesystem stack**: a shared `no_std`
//! substrate under thin, clean-room format frontends (the *flock*). This crate
//! is the substrate (L2) — the shared engine every frontend sits on:
//!
//! * [`codec`] — the decompression-codec registry (deflate/lz4 wired; zstd/xz/lzo
//!   declared), one decoder shared across all compressed formats.
//! * [`BlockCache`] — an immutable decompressed-block LRU (read-only ⇒ no
//!   invalidation).
//! * [`read_cap`] — bounded-allocation hardening (no OOM on a hostile size).
//! * [`FoldFrontend`] — the trait the flock implements; [`Verifier`] — the
//!   shepherd (integrity-verification seam).
//! * [`BlockSource`] — the byte source a frontend reads over (LamBoot adapts its
//!   own `BlockSource` to this at integration).
//!
//! Frontends (`lamfold-iso`, `-udf`, `-squash`, `-erofs`, …) live in their own
//! crates and depend on this one. See `the lamfold design spec`.
//!
//! Read-only by construction; `#![forbid(unsafe_code)]` in the substrate —
//! `zerocopy` removes the transmute class, so the parse layer needs no `unsafe`.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod cache;
mod codec;
mod error;
mod frontend;
mod read_cap;
mod source;
mod verify;

pub use cache::BlockCache;
pub use codec::{decode, Codec};
pub use error::{FoldError, Result};
pub use frontend::{DirEntry, FileKind, FoldFrontend, Metadata, NodeId, SubstrateCtx};
pub use read_cap::{
    checked_block_len, checked_full_read_len, MAX_BOOT_FILE_BYTES, MAX_DECOMPRESSED_BLOCK_BYTES,
};
pub use source::{BlockSource, SliceSource};
pub use verify::{NoVerifier, Verifier};
