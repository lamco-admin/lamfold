# lamfold-iso

[![Crates.io](https://img.shields.io/crates/v/lamfold-iso.svg)](https://crates.io/crates/lamfold-iso)
[![Docs.rs](https://docs.rs/lamfold-iso/badge.svg)](https://docs.rs/lamfold-iso)
[![CI](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml/badge.svg)](https://github.com/lamco-admin/lamfold/actions/workflows/ci.yml)
[![Website](https://img.shields.io/badge/website-lamco.ai-blue.svg)](https://lamco.ai/products/lamboot/)

> The optical frontend of the [`lamfold`](../lamfold) read-only media stack:
> Clean-room ISO 9660 with Rock Ridge, Joliet, El Torito and zisofs.

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so it
composes through LamBoot's `dispatch_fs_over_source`, including recursively over
a file inside another volume (an `.iso` inside a partition). Supersedes the
earlier `lamoptical` scaffold. Spec: `the lamfold design spec` §4.

- **Website:** [LamBoot](https://lamco.ai/products/lamboot/), the bootloader lamfold is written for
- **Documentation:** [docs.rs/lamfold-iso](https://docs.rs/lamfold-iso) and the [LamBoot docs](https://lamco.ai/products/lamboot/docs/)
- **Repository:** [github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold)
- **Company:** [Lamco Development LLC](https://lamco.ai/)

## What it reads

| Layer | State |
| ----- | ----- |
| ISO 9660 / ECMA-119 base: PVD, directory records, file extents | Read |
| Rock Ridge: real names, symlinks through `read_link`, POSIX mode, continuation areas (`CE`) for long names and link targets | Read |
| Joliet (UCS-2 supplementary descriptor; Rock Ridge, then Joliet, then the base names) | Read |
| El Torito: the UEFI boot image through `el_torito_uefi_image()`, sized from its FAT boot sector when the catalog's count is 0 | Read |
| zisofs (paged zlib through the substrate codec, `zisofs` feature) | Read |
| `CL`/`PL`/`RE` deep-directory relocation; multi-extent files of 4 GiB or more | Not yet |

Tested against `xorriso` images: plain ISO 9660, `-R`, `-J`, `-e` UEFI El Torito,
`--zisofs`, and (since 0.1.1) a Rock Ridge tree whose empty files and symlinks
share one extent address, with a 250-character name and a long link target, and
an appended EFI partition entered the way archiso enters it. `#![forbid(unsafe_code)]`;
every on-disk field is read through bounds-checked helpers and every allocation
through the substrate read cap.

## Installation

```sh
cargo add lamfold-iso
```

## Clean-room posture

Derived only from ECMA-119 / IEEE P1281+P1282 / El Torito specs and the permissive
references in `NOTICE`. The GPL implementations (libcdio, Linux `fs/isofs`) are
fenced off: References and test oracles only, never copied.

## Build / test

```bash
cargo build
cargo test --all-features                    # every fixture in tests/fixtures/
cargo build --no-default-features            # no_std check
```

## Contributing

Issues and pull requests are welcome at
[github.com/lamco-admin/lamfold](https://github.com/lamco-admin/lamfold). A
change to the read path should come with an `xorriso` fixture
(`tests/fixtures/regenerate-xorriso.sh`).

## License

MIT OR Apache-2.0, copyright Lamco Development LLC.
