# Changelog

All notable changes to the lamfold substrate are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Each frontend
crate keeps its own changelog.

## [0.1.4] - 2026-10-15

### Changed

- Dependencies are minimums at the versions in the Debian archive instead of
  exact pins, so Debian builds lamfold against whatever its archive holds and
  crates.io users get bug-fix releases (`miniz_oxide` 0.9.1, `lz4_flex`
  0.13.0, `lzokay` 2.0.1, `sha2` 0.11.0). Exact pins also forced every
  consumer down to them: LamBoot's `lz4_flex` went from 0.13.1 back to
  0.13.0.
- `lzma-rust2` 0.16.4 to 0.19.0, the version in Debian's archive. The LZMA
  and xz call sites are unchanged.

## [0.1.3] - 2026-10-15

### Security

- ruzstd 0.9.1, the first release with the fix for RUSTSEC-2026-0344, in place
  of the exact 0.8.3 pin. The requirement is now a caret from 0.9.1. The zstd
  codec's call sites are unchanged.

### Changed

- The description names Lamco Development LLC, the README gains website,
  documentation, install and contributing sections, and CI also runs weekly.

## [0.1.2] - 2026-08-19

### Changed

- Dependencies aligned with the versions in the Debian archive.

## [0.1.1] - 2026-06-06

### Added

- `microlzma_block_decode`, for EROFS MicroLZMA pclusters.

## [0.1.0] - 2026-06-06

First 0.1.x release: the codec registry, block cache, bounded reads, the
frontend trait and the verifier seam. (0.0.1 and 0.0.2 were the same stack
before it moved to 0.1.x so that caret requirements work.)
