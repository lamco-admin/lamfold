# lamfold-iso

> The optical frontend of the [`lamfold`](../lamfold) read-only media stack —
> clean-room ISO9660 (+ Rock Ridge / Joliet / El Torito / zisofs).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so it
composes through LamBoot's `dispatch_fs_over_source` — including recursively over
a file inside another volume (an `.iso` inside a partition). Supersedes the
earlier `lamoptical` scaffold. Spec: `the lamfold design spec` §4.

## Status — optical frontend

| Layer | State |
| ----- | ----- |
| **ISO9660 / ECMA-119 base** — PVD, directory records, file extents, multi-sector reads | ✅ done + tested against a real `xorriso` ISO |
| **Rock Ridge** (SUSP/RRIP — real names, symlinks, POSIX) | ⏳ S1-cont |
| **Joliet** (UCS-2 supplementary descriptor) | ⏳ S1-cont |
| **El Torito** (boot catalog → UEFI image side-channel) | ⏳ S1-cont |
| **zisofs** (transparent Deflate, via the substrate codec) | ⏳ S1-cont |

Verified: builds default + `--no-default-features` (`no_std`); integration test
walks a real plain-ISO9660 image end to end (probe, tree walk, small +
multi-sector file reads, cross-sector offset reads, missing-entry lookup).
`#![forbid(unsafe_code)]` — every on-disk field read through bounds-checked
little-endian helpers; every allocation through the substrate read cap.

## Clean-room posture

Derived only from ECMA-119 / IEEE P1281+P1282 / El Torito specs and the permissive
references in `NOTICE`. The GPL implementations (libcdio, Linux `fs/isofs`) are
fenced off — references and test oracles only, never copied.

## Build / test

```bash
cargo build
cargo test                                   # walks tests/fixtures/base.iso
cargo build --no-default-features            # no_std check
```

MIT OR Apache-2.0.
