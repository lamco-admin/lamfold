# lamfold

> `no_std` read-only media filesystem **stack** — the substrate core.

A layered, integrity-verifying, read-only filesystem stack: a shared substrate
under thin, clean-room format frontends (the *flock*). The name is a sheepfold —
the enclosure that holds the flock of formats — and the layered/recursive
structure it reads. Spec: `the lamfold design spec`.

**This crate is the substrate (L2).** Format frontends (`lamfold-iso`, `-udf`,
`-squash`, `-erofs`, …) live in their own crates and depend on this one.

## Status — substrate core: real, building, host-tested

| Component | State |
| --------- | ----- |
| `read_cap` — bounded-allocation hardening (no OOM on hostile sizes) | ✅ done + tested |
| `BlockCache` — immutable decompressed-block LRU | ✅ done + tested |
| `codec` registry — **deflate (raw+zlib), lz4 (block), zstd, xz, lzo** | ✅ all five wired + round-trip tested |
| `BlockSource` / `SliceSource` — the byte source | ✅ done + tested |
| `FoldFrontend` trait — the flock contract | ✅ defined |
| `Verifier` (the shepherd) seam + `NoVerifier` | ✅ seam in; real fs-verity/composefs landed |

Verified: builds with default features **and** `--no-default-features` (genuine
`no_std`); 18 unit tests pass (all 5 codecs round-trip); `#![forbid(unsafe_code)]`.

## Codecs (all permissive `no_std`, opt-in per cargo feature)

`miniz_oxide` (deflate/zlib), `lz4_flex` (lz4 block), `ruzstd` (zstd), `lzma-rust2`
(xz/lzma), `lzokay` (lzo). Parse: `zerocopy`. See `the lamfold design spec` §6
for the pinned set — zero copyleft anywhere in the tree.

## Build

```bash
cargo build              # default: std (host) + deflate + lz4
cargo test               # host tests
cargo build --no-default-features --features codec-deflate,codec-lz4   # no_std check
```

MIT OR Apache-2.0. The six-layer testing + fuzzing bar (`the lamfold design spec` §11) gates
every frontend before any "functional" claim.
