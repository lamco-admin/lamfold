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
| **ISO9660 / ECMA-119 base** — PVD, directory records, file extents, multi-sector reads | ✅ done + tested |
| **Rock Ridge** (SUSP/RRIP — real names, symlinks via `read_link`, POSIX mode) | ✅ done + tested |
| **Joliet** (UCS-2 supplementary descriptor; RR > Joliet > base preference) | ✅ done + tested |
| **El Torito** (boot catalog → `el_torito_uefi_image()` side-channel) | ✅ done + tested |
| **zisofs** (transparent paged-zlib via the substrate codec) | ✅ done + tested (`zisofs` feature) |
| Rock Ridge `CE` continuation / `CL`/`PL`/`RE` deep-dir relocation; multi-extent files | ⏳ future |

**Complete.** Verified against **real** `xorriso`-generated images (plain
ISO9660, `-R`, `-J`, `-e` UEFI El Torito, `--zisofs`): probe, tree walks, real
names, symlinks, the UEFI boot image, UCS-2 long names, and transparent zisofs
decompression. Builds default + `--no-default-features` (with and without
`zisofs`), `no_std`. `#![forbid(unsafe_code)]` — every on-disk field read through
bounds-checked little-endian helpers; every allocation through the substrate read
cap; decompression through the shared substrate codec.

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
