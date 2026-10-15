# Changelog

All notable changes to lamfold-iso are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2] - 2026-10-15

### Added

- **Multi-extent files**, how ISO 9660 records a file of 4 GiB or more: The
  records flagged Multi-Extent and the last one become one node whose size is
  the sum of the extents, and reads continue across them with 64-bit offsets.
  0.1.1 listed one entry per extent and read only the first.

## [0.1.1] - 2026-10-15

### Fixed

- On `xorriso` images (what most distributions ship) every empty file and
  every symlink shares one extent address, and nodes were keyed by that
  address, so they all became the first one: Every symlink read as the first
  link's target or as an empty file. Nodes are now keyed by their directory
  record.
- Rock Ridge continuation areas (`CE`) are followed, so names and link targets
  too long for their directory record come back whole; a 250-character name
  had come back as its first 93 bytes. Component continuation is honoured too.
- An absolute symlink read as `//usr/bin/bash`; it now reads `/usr/bin/bash`.
- The UEFI boot image of an ISO whose El Torito catalog records a sector count
  of 0 (Arch's, whose EFI image is too large for the 16-bit count) came back
  zero-length. Its size now comes from the image's FAT boot sector.

### Changed

- The description names Lamco Development LLC, and the README gains the
  current feature table and website, documentation, install and contributing
  sections.

## [0.1.0] - 2026-06-06

First 0.1.x release: ISO 9660 with Rock Ridge, Joliet, El Torito and zisofs.
(0.0.1 and 0.0.2 were the same stack before it moved to 0.1.x.)
