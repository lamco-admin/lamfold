# lamfold-squash

[![Crates.io](https://img.shields.io/crates/v/lamfold-squash.svg)](https://crates.io/crates/lamfold-squash)
[![Docs.rs](https://docs.rs/lamfold-squash/badge.svg)](https://docs.rs/lamfold-squash)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> The SquashFS frontend of the [`lamfold`](../lamfold) read-only media stack:
> Clean-room SquashFS 4.0 (gzip, xz, zstd, lz4, lzo).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so it
composes through LamBoot's `dispatch_fs_over_source`, including recursively over
a file inside another volume. SquashFS is the root filesystem inside almost every
Linux live ISO, so this frontend reads the live root straight out of an ISO.
Spec: `the lamfold design spec` §4.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold-squash](https://docs.rs/lamfold-squash) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

## What it reads

| Layer | State |
| ----- | ----- |
| Superblock, compressed metadata blocks, basic and extended directory, file and symlink inodes, directory listings | Read |
| Data blocks (compressed or stored), sparse blocks as zeros, fragment tails | Read |
| **`read_at` decodes only the blocks it touches**, so a file of any size reads in pieces | Read (0.1.1) |
| xattrs, the export and lookup tables, the directory index (listings are scanned) | Not used |

Legacy `lzma1` (id 2) is refused with `Unsupported`. Verified against real
`mksquashfs` images, including a 600 MiB sparse file larger than lamfold's
whole-file read cap. `#![forbid(unsafe_code)]`.

## Compression features

A SquashFS image uses one compressor; each maps to a substrate codec behind an
opt-in feature (all on by default):

| feature | compression id | substrate codec |
| ------- | -------------- | --------------- |
| `gzip`  | 1 | `Codec::Zlib` (miniz_oxide) |
| `lzo`   | 3 | `Codec::Lzo` (lzokay) |
| `xz`    | 4 | `Codec::Xz` (lzma-rust2) |
| `lz4`   | 5 | `Codec::Lz4` (lz4_flex) |
| `zstd`  | 6 | `Codec::Zstd` (ruzstd) |

An image whose compressor is disabled fails with `Unsupported` rather than
misreading.

## Installation

```sh
cargo add lamfold-squash
```

## Clean-room posture

Derived only from the public SquashFS 4.0 format documentation and the
permissive references in `NOTICE`. The GPL implementations (Linux `fs/squashfs`,
`squashfs-tools`) are fenced off: Never read or copied; squashfs-tools only
builds the fixtures.

## Build / test

```bash
cargo build
cargo test                                   # every fixture in tests/fixtures/
cargo build --no-default-features            # no_std check
```

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold). A
change to the read path should come with a fixture built by a real tool
(`mksquashfs`, as `tests/fixtures/regenerate-large.sh` does).

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
