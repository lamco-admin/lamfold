# Changelog

All notable changes to lamfold-cpio are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2] - 2026-10-15

### Added

- **Compressed initramfs members read.** gzip, zstd, xz, legacy `.lzma` and LZ4
  legacy-frame members are decoded with `lamfold::decode_stream` and their
  archives folded into the tree, and an archive after a compressed member is
  found where the member ends. 0.1.1 refused every compressed member. Debian
  13's own initramfs now reads, every file identical to what `unmkinitramfs`
  extracts. The decompressed members together are capped at 512 MiB
  (`lamfold::MAX_BOOT_FILE_BYTES`).
- Features `gzip`, `zstd`, `xz` and `lz4` (on by default) select the codecs.

### Changed

- bzip2 and lzop members are refused as `Unsupported("stream_bzip2")` and
  `Unsupported("stream_lzo")` (they were `cpio_compressed_member`), and a
  compressed archive inside a decompressed one is corrupt, as the kernel
  treats it.
- Zero bytes between archives are skipped one at a time, and an archive must
  start at a 4-byte-aligned offset, as the kernel requires.

## [0.1.1] - 2026-10-15

### Fixed

- **Only the first archive of an initramfs was read.** Initramfs images are
  often an uncompressed early archive (CPU microcode) followed by the main
  one, and reading stopped at the first `TRAILER!!!`, so `/init` and
  everything else in the main archive were missing, with no error. Every
  newc or CRC archive in the chain is read now, skipping the zero padding
  between them as the kernel does.
- **Hard links read as empty files.** newc stores a hard-linked file's data
  once, on its last link; every link reads it now.
- **A compressed archive in the chain** (most distributions compress the main
  one) is refused as `Unsupported("cpio_compressed_member")` instead of
  leaving the tree with only the microcode archive.

### Added

- CRC-format (`070702`) regular files are checked against their checksum
  (`Corrupt("cpio_bad_crc")`), and anything other than padding between
  archives is `Corrupt("cpio_junk_between_archives")`.
- Fixtures built with GNU cpio (`tests/fixtures/regenerate.sh`); each test
  fails on 0.1.0.

### Changed

- `cpio_reader` is required from 0.1.2 rather than at exactly 0.1.2.
- The description names Lamco Development LLC, and the README gains website,
  documentation, install and contributing sections.

## [0.1.0] - 2026-06-06

- First release: a `FoldFrontend` tree over `cpio_reader`.
