# lamfold-cpio

> The initramfs/cpio frontend of the [`lamfold`](../lamfold) read-only media
> stack — a `FoldFrontend` tree over the permissive `cpio_reader`.

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`. cpio
is the Linux initramfs/initrd archive format — a flat `(header, path, data)`
stream, not a block filesystem. Spec: `the lamfold design spec` §4.

## Reuse, not clean-room

cpio's record parsing is trivial and a permissive `no_std` reader already exists,
so — per the SDS "reuse where licensing does not hinder" rule — this frontend
**reuses `cpio_reader`** (MIT OR Apache-2.0) for record parsing and contributes
only what lamfold needs: fold the flat path list into a directory tree (creating
intermediate directories a nested path implies) and present it through
`FoldFrontend`, so an initramfs reads through the same `dispatch_fs_over_source`
path as a real filesystem.

## Status

| Layer | State |
| ----- | ----- |
| **Record parsing** (newc / CRC / portable-ASCII / old-binary, via `cpio_reader`) | ✅ reused |
| **Tree construction** (explicit + path-implied directories) | ✅ done + tested |
| **Regular files** (zero-copy `(offset, len)` views into the archive) | ✅ done + tested |
| **Symlinks** via `read_link` | ✅ done + tested |

**Complete.** Verified against a **real** `cpio -H newc` archive: probe, tree
build (including a directory implied by `sub/inner.txt`), files (full + partial +
EOF), and a symlink. The archive is read into memory once (read-capped) —
appropriate for an initramfs, which is resident anyway. `#![forbid(unsafe_code)]`.

## Build / test

```bash
cargo build
cargo test                                   # walks tests/fixtures/initramfs.newc
cargo build --no-default-features            # no_std check
```

MIT OR Apache-2.0.
