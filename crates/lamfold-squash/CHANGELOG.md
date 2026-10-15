# Changelog

All notable changes to lamfold-squash are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-10-15

### Fixed

- **Files over 512 MiB couldn't be read at all.** Every `read_at` decoded the
  whole file, so it was refused past lamfold's whole-file read cap, and a
  small read of a large file decoded all of it. Only the blocks a read
  touches are decoded now: A 600 MiB file's last bytes read without the file
  in memory.

### Changed

- The description names Lamco Development LLC; the README gains website,
  documentation, install and contributing sections; the crate ships this
  changelog.

## [0.1.0] - 2026-06-06

- First release: SquashFS 4.0 over the lamfold substrate.
