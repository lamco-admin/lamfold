# Changelog

All notable changes to lamfold-udf are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-10-15

### Added

- **UDF 2.01, mkudffs's default revision.** Its File Entries are Extended File
  Entries (tag 266), which were refused as unsupported; they read now. The
  fixture is a mkudffs 2.01 image the kernel populated.

### Changed

- The description names Lamco Development LLC; the README gains website,
  documentation, install and contributing sections; the crate ships this
  changelog.

## [0.1.0] - 2026-06-06

- First release: UDF 1.02 over the lamfold substrate.
