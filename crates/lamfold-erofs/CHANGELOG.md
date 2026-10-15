# Changelog

All notable changes to lamfold-erofs are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.3] - 2026-10-15

### Added

- **Chunk-based files** (`mkfs.erofs --chunksize`, composefs-style images): The
  4-byte block map after the inode locates each chunk, and a chunk at the null
  address is a hole that reads as zeros. 0.1.2 refused every such file.

### Fixed

- The compression configs are found after the superblock's extension slots
  (`sb_extslots`) rather than at a fixed offset.
- Linux 7.2's hole pclusters (the HEAD lcluster's hole bit in a full index, or
  a zero block count in a compact one) are refused rather than decoded as data.
  No released erofs-utils writes them yet.
- 8-byte chunk indexes, which mkfs.erofs writes only for chunks on a blob
  device, are refused with `Unsupported`.

## [0.1.2] - 2026-10-15

### Fixed

- LZ4 files could read back wrong with no error. On an `LZ4_0PADDING` image
  (every default lz4 and lz4hc image) a pcluster's stream starts at its first
  non-zero byte, and 0.1.1 tried offset 0 first, which also decodes
  "successfully" when there is one padding byte. Real executables hit this
  (`/bin/bash` came back with 1,740 wrong bytes). The stream now starts where
  the kernel starts it.
- Files whose compact index starts on a 32-byte boundary, as a 52- or 56-byte
  inline xattr area (an SELinux label) puts it, had every big pcluster placed
  one block too far: Wrong bytes or a decode error with zstd, lzma and deflate.
- Images with fragments or all-fragments (Fedora's live images, lorax) and
  compressed directories (`--zD`) read wrong with no error, or as an empty
  tree. They are refused at open with `Unsupported` until the packed inode is
  implemented.
- LZMA streams built with custom lc/lp/pb (erofs-utils 1.9) failed. The
  properties now come from each stream's first byte.
- Compressed files on images with blocks smaller than 4 KiB failed, because
  the logical cluster size was taken as 4 KiB. It now follows the block size.

### Changed

- `open` refuses what can't be read anywhere in the image: Unknown incompat
  features, the 48-bit layout, metabox and extra devices. Interlaced pclusters
  are refused per file.
- The description names Lamco Development LLC, and the README gains the
  current feature table and website, documentation, install and contributing
  sections.

### Added

- Fixtures built by erofs-utils 1.9.4 for each case above
  (`tests/fixtures/regenerate.sh`); every one fails on 0.1.1.

## [0.1.1] - 2026-06-06

### Added

- Compressed files with the compact index and big pclusters, and the lzma,
  deflate and zstd codecs.

## [0.1.0] - 2026-06-06

The uncompressed read path, compressed files with the full (legacy) index and
lz4, and the shepherd. (0.0.1 and 0.0.2 were the same stack before it moved to
0.1.x so that caret requirements work.)
