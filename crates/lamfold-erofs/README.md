# lamfold-erofs

> The EROFS frontend of the [`lamfold`](../lamfold) read-only media stack —
> clean-room EROFS (uncompressed) **plus the shepherd**, the fs-verity integrity
> story.

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`.
EROFS is the forward standard for immutable images — the metadata format
**composefs** builds on for content-addressed OS (bootc/Silverblue) and OCI
images — so this is where the lamfold stack meets the shepherd: read the
metadata here, anchor trust in lamfold's native fs-verity Merkle. Spec:
`the lamfold design spec` §4 + §7.

## Status — forward standard + the integrity differentiator

| Layer | State |
| ----- | ----- |
| **Superblock** (byte 1024, magic `0xE0F5E1E2`, blkszbits, root nid, meta blkaddr) | ✅ done + tested |
| **Inodes** — compact (32 B) + extended (64 B), POSIX mode → kind | ✅ done + tested |
| **Data layouts** — `FLAT_PLAIN` + `FLAT_INLINE` (full blocks + inline tail) | ✅ done + tested |
| **Directories** — `erofs_dirent` array + names, multi-block | ✅ done + tested |
| **Symlinks** via `read_link` (inline target) | ✅ done + tested |
| **The shepherd** — `MerkleVerifier` gates every data block read; tampered media → `VerifyFailed` | ✅ done + tested |
| Compressed clusters (lz4 / lzma / zstd / deflate), chunk-based files, xattrs, shared-xattr area | ⏳ second stage |

**Complete (uncompressed path + integrity).** Verified against a **real**
`mkfs.erofs` image: probe, tree walk, pure-inline files, a 300 KB file across 73
full blocks + a 992 B inline tail (full, partial across the boundary, EOF), a
nested directory, and a symlink. The shepherd is exercised end to end — a
`MerkleVerifier` built over genuine content verifies all 74 blocks as they are
read, and a single flipped byte on the medium is refused before any data is
surfaced. Builds default + `--no-default-features` (`no_std`), clippy
`-D warnings` clean, `#![forbid(unsafe_code)]`.

## The shepherd (integrity)

The `verity` feature (on by default) pulls `lamfold/verify`, the substrate's
native fs-verity SHA-256 Merkle (RustCrypto `sha2`). `lamfold::fsverity_digest_sha256`
reproduces the exact measurement of the `fsverity` userspace tool — the digest
composefs/bootc sign — and `lamfold::MerkleVerifier` implements the substrate
`Verifier` seam so any frontend's reads can be trust-gated. Put one in the
`SubstrateCtx` and tampered media fails the read; use `NoVerifier` for unverified
media.

## Clean-room posture

Derived only from the public EROFS format docs. The on-disk header
`fs/erofs/erofs_fs.h` is itself **SPDX MIT**, so its struct layout is referenced
directly; the GPL-2 EROFS *driver* (`fs/erofs/*.c`) is fenced off — never read
or copied.

## Build / test

```bash
cargo build
cargo test                                   # walks tests/fixtures/erofs.img
cargo build --no-default-features            # no_std check
```

MIT OR Apache-2.0.
