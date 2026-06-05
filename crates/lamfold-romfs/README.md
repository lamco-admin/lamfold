# lamfold-romfs

> The romfs frontend of the [`lamfold`](../lamfold) read-only media stack —
> clean-room Linux romfs (`-rom1fs-`).

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`. romfs
is the minimal uncompressed read-only filesystem for tiny embedded and
early-boot images — the floor of the flock. Spec:
`the lamfold design spec` §4.

## Status — legacy/embedded adds

| Layer | State |
| ----- | ----- |
| **Header** (`-rom1fs-`, big-endian size, volume name) | ✅ done + tested |
| **File headers** (16-byte aligned; `next` low-3-bits = type, `info`, `size`) | ✅ done + tested |
| **Directory next-chain** (`info` = first child; `.`/`..` skipped) | ✅ done + tested |
| **Regular files** (uncompressed, contiguous) | ✅ done + tested |
| **Symlinks** via `read_link` | ✅ done + tested |

**Complete.** Verified against a **real** `genromfs` image: probe, tree walk,
files (full + partial + EOF), a nested directory, and a symlink. Builds default +
`--no-default-features` (`no_std`), clippy `-D warnings` clean,
`#![forbid(unsafe_code)]` — every field read big-endian through bounds-checked
helpers.

## Clean-room posture

Derived only from the kernel romfs format docs. The GPL-2 `fs/romfs` driver is
fenced off. **`neotron-romfs` is a different, incompatible format and GPL-3** —
deliberately not consulted (a trap noted in the SDS landscape).

## Build / test

```bash
cargo build
cargo test                                   # walks tests/fixtures/romfs.img
cargo build --no-default-features            # no_std check
```

MIT OR Apache-2.0.
