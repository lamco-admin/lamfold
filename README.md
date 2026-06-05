# lamfold

> A `no_std`, integrity-verifying **read-only media filesystem stack** — a shared
> substrate under thin, clean-room format frontends.

This is a **metacrate workspace** (one repo, many published crates), the same
shape as the `lamco-wayland` / `lamco-rdp` workspaces. The name is a sheepfold —
the enclosure holding the *flock* of formats — and the layered, recursive
structure it reads. Authoritative spec: `the lamfold design spec`.

## Crates

| Crate | Role | Status |
| ----- | ---- | ------ |
| [`crates/lamfold`](crates/lamfold) | **substrate core** — codec registry (deflate/lz4/zstd/xz/lzo), immutable-block cache, bounded zero-copy parse + read-cap, `FoldFrontend` trait, the shepherd (`Verifier` seam + native fs-verity Merkle) | ✅ substrate + shepherd (21 tests) |
| [`crates/lamfold-iso`](crates/lamfold-iso) | **optical frontend** — ISO9660 + Rock Ridge + Joliet + El Torito + zisofs | ✅ complete |
| [`crates/lamfold-udf`](crates/lamfold-udf) | **UDF frontend** — ECMA-167 / OSTA 1.02 read | ✅ base (real mkudffs image) |
| [`crates/lamfold-squash`](crates/lamfold-squash) | **compressed frontend** — SquashFS 4.0 (gzip/xz/zstd/lz4/lzo), the live-ISO root | ✅ complete (real mksquashfs image) |
| [`crates/lamfold-erofs`](crates/lamfold-erofs) | **forward-standard frontend** — EROFS (uncompressed) + the shepherd (fs-verity block gating) | ✅ complete (real mkfs.erofs image) |
| [`crates/lamfold-cramfs`](crates/lamfold-cramfs) | **legacy compressed frontend** — cramfs (per-page zlib) | ✅ complete (real mkcramfs image) |
| [`crates/lamfold-romfs`](crates/lamfold-romfs) | **minimal-embedded frontend** — Linux romfs (`-rom1fs-`) | ✅ complete (real genromfs image) |
| [`crates/lamfold-cpio`](crates/lamfold-cpio) | **initramfs frontend** — newc cpio tree (reuses `cpio_reader`) | ✅ complete (real cpio archive) |

Every frontend is a thin member that depends on `lamfold` (the namesake core,
like the `lamco-wayland` crate within the `lamco-wayland` workspace) and contains
*only* on-disk structure parsing — codecs, caching, bounded parse, and
verification all come from the substrate. Each member publishes as its own crate
(Debian/crates.io granularity) while versioning and CI'ing together.

## Use

Not yet on crates.io — development is in progress. Until the first release,
depend on the member you need via this Git repository:

```toml
# the optical frontend (ISO9660 + Rock Ridge / Joliet / El Torito / zisofs)
lamfold-iso   = { git = "https://github.com/lamco-admin/lamfold", default-features = false, features = ["zisofs"] }
# the EROFS frontend + the shepherd (fs-verity integrity)
lamfold-erofs = { git = "https://github.com/lamco-admin/lamfold", default-features = false, features = ["verity"] }
# …and lamfold-udf / -squash / -cramfs / -romfs / -cpio, plus the `lamfold` substrate core.
```

Once published, the same members will be available as crates.io versions
(`lamfold-iso = "X.Y.Z"`, …) — see each crate's `documentation` link.

## Build / test (whole workspace)

```bash
cargo test                                   # all members
cargo build --no-default-features            # no_std check
cargo clippy --all-targets -- -D warnings
```

The pinned, all-permissive dependency set lives once in the workspace root's
`[workspace.dependencies]` (zero copyleft in the tree). MIT OR Apache-2.0.
