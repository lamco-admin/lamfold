# lamfold-squash

> The SquashFS frontend of the [`lamfold`](../lamfold) read-only media stack —
> clean-room SquashFS 4.0 (gzip / xz / zstd / lz4 / lzo).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so it
composes through LamBoot's `dispatch_fs_over_source` — **including recursively**
over a file inside another volume. SquashFS is the root filesystem inside almost
every Linux live ISO, so this frontend is the recursion payoff: mounted over the
squashfs-root *file inside* an ISO9660 image (a `FileBlockSource` loopback), it
reads the live root directly. Spec: `the lamfold design spec` §4.

## Status — compressed read-only frontend

| Layer | State |
| ----- | ----- |
| **Superblock** — magic, block size, compression id → substrate codec, table offsets | ✅ done + tested |
| **Compressed metadata blocks** — 2-byte header, substrate decompress, cross-block spans | ✅ done + tested |
| **Inodes** — basic + extended directories, files, symlinks | ✅ done + tested |
| **Directory listings** — header runs + entries, child inode-ref reconstruction | ✅ done + tested |
| **Data blocks** — per-block compressed/uncompressed flag, multi-block reassembly, sparse zero-fill | ✅ done + tested |
| **Fragments** — the shared tail-block table, fragment read + decompress | ✅ done + tested |
| `read_link` (symlink targets) | ✅ done |
| xattrs, export/lookup table, directory index (we linear-scan), streaming per-block `read_at` | ⏳ future |

**Complete.** Verified against a **real** `mksquashfs -comp gzip` image:
probe, tree walk, a nested directory, two fragment-packed small files, and a
300 KB three-block file (full + partial reads, EOF). Every compressed metadata,
data, and fragment block is decompressed through the shared substrate codec.
Builds default + `--no-default-features`, `no_std`, `#![forbid(unsafe_code)]` —
every on-disk field read through bounds-checked little-endian helpers, every
allocation through the substrate read cap.

## Compression features

A SquashFS image is built with exactly one compressor; each maps to a substrate
codec behind an opt-in cargo feature (all on by default):

| feature | compression id | substrate codec |
| ------- | -------------- | --------------- |
| `gzip`  | 1 | `Codec::Zlib` (miniz_oxide) |
| `lzo`   | 3 | `Codec::Lzo` (lzokay) |
| `xz`    | 4 | `Codec::Xz` (lzma-rust2) |
| `lz4`   | 5 | `Codec::Lz4` (lz4_flex) |
| `zstd`  | 6 | `Codec::Zstd` (ruzstd) |

Legacy `lzma1` (id 2) is rejected with `Unsupported`. Disable the codecs you
don't need to shrink the binary; an image whose compressor is disabled fails
cleanly with `Unsupported` rather than mis-reading.

## Clean-room posture

Derived only from the public SquashFS 4.0 format docs and the permissive
references in `NOTICE`. The GPL implementations (Linux `fs/squashfs`,
`squashfs-tools`) are fenced off — never read or copied.

## Build / test

```bash
cargo build
cargo test                                   # walks tests/fixtures/gzip.sqfs
cargo build --no-default-features            # no_std check
```

MIT OR Apache-2.0.
