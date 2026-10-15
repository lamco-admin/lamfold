# lamfold-cpio

[![Crates.io](https://img.shields.io/crates/v/lamfold-cpio.svg)](https://crates.io/crates/lamfold-cpio)
[![Docs.rs](https://docs.rs/lamfold-cpio/badge.svg)](https://docs.rs/lamfold-cpio)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> The initramfs and cpio frontend of the [`lamfold`](../lamfold) read-only media
> stack.

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so an
initramfs reads through LamBoot's `dispatch_fs_over_source` like a filesystem.
cpio is a flat `(header, path, data)` stream; this frontend folds it into a
directory tree. Spec: `the lamfold design spec` §4.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold-cpio](https://docs.rs/lamfold-cpio) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

## What it reads

| Layer | State |
| ----- | ----- |
| **Initramfs chains**: Several newc or CRC archives back to back, with the zero padding between them, as initramfs-tools and dracut write them | Read, as the kernel unpacks them |
| **Hard links** (newc stores the data once, on the last link) | Every link reads the data |
| **CRC archives** (`070702`) | Regular files' checksums verified |
| **odc and old binary archives** (single archive, through `cpio_reader`) | Read |
| Tree construction (explicit and path-implied directories), regular files as zero-copy views, symlinks via `read_link` | Read |
| **A compressed archive in the chain** (gzip, bzip2, LZMA, xz, LZO, LZ4, zstd) | Refused: `Unsupported("cpio_compressed_member")` |

Most distributions compress the main archive of their initramfs, so today
lamfold-cpio refuses those rather than showing only the uncompressed microcode
archive in front of it. Reading them needs a streaming decoder in `lamfold`.

The whole input is read into memory once (read-capped), which suits an
initramfs. `#![forbid(unsafe_code)]`.

## Installation

```sh
cargo add lamfold-cpio
```

## Reuse

The newc and CRC records an initramfs uses are parsed here, since an initramfs
is several archives and the parser has to know where each one ends. The odc and
old binary formats reuse [`cpio_reader`](https://crates.io/crates/cpio_reader)
(MIT OR Apache-2.0).

## Build / test

```bash
cargo build
cargo test                                   # every fixture in tests/fixtures/
cargo build --no-default-features            # no_std check
```

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold). A
change to the read path should come with a GNU cpio fixture
(`tests/fixtures/regenerate.sh`).

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
