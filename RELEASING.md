# Releasing lamfold

**Core rule: republish a crate only when *its own* code changes.** A fix in the
`lamfold` substrate must not drag the unchanged frontends along.

## Why the versioning is set up the way it is

Two earlier mistakes are designed out:

1. **No lockstep version.** `[workspace.package]` carries shared metadata
   (edition, license, authors, repo, MSRV) but **not** `version`. Each crate in
   `crates/*` declares its **own** `version`, so a one-crate fix bumps one crate.
2. **Caret internal dep, not exact.** Frontends depend on the substrate via the
   workspace dep `lamfold = { …, version = "0.1", … }` → caret `^0.1` accepts any
   `0.1.x`. So a substrate patch (`0.1.0 → 0.1.1`) is picked up by the **already
   published** frontends with **no frontend republish**.
   - At `0.0.x`, cargo's caret treats every version as breaking (`"0.0.2"` means
     *exactly* 0.0.2). That is why the stack lives at `0.1.x`, not `0.0.x` — so
     normal caret semantics work. (History: 0.0.1/0.0.2 exist on crates.io; the
     stack rebased to 0.1.0 to escape `0.0.x` exact-caret.)

## Procedure

### Substrate-only fix (the common case — e.g. read_cap, codec, cache)
1. Bump **only** `crates/lamfold/Cargo.toml` `version` (e.g. `0.1.0 → 0.1.1`).
2. `cargo test` (full workspace), then `cargo publish -p lamfold`.
3. In the consumer (`lamboot-core/Cargo.toml`), raise **only** the `lamfold`
   minimum (`"0.1.0"` to `"0.1.1"`) and run `cargo update -p lamfold`. The
   frontend requirements stay put; their `^0.1` dep resolves to the new
   substrate. **Do not touch the frontends.**

### A frontend's own code changes (e.g. lamfold-iso parser fix)
1. Bump **only** that frontend's `version`.
2. `cargo publish -p <that-frontend>`.
3. Raise only that frontend's minimum in the consumer and `cargo update -p` it.

### A breaking substrate change (rare)
A `0.1 → 0.2` substrate bump *does* require republishing every frontend (their
`^0.1` no longer matches) and bumping their `^0.2` dep. Reserve `0.x` minor bumps
for genuinely breaking substrate API changes; use patch bumps (`0.1.z`) for
everything else.

## Consumer pinning

`lamboot-core` requires each crate as a minimum version (`"0.1.1"`), not an
exact pin (LamBoot decision 37, 2026-10-15). Debian builds LamBoot offline
against whatever its archive holds, and an exact pin breaks that build whenever
the archive moves. LamBoot's `Cargo.lock` records exactly what it builds, so a
new release only reaches LamBoot through a deliberate `cargo update -p`. The
workspace's own third-party dependencies are minimums at the Debian archive's
versions for the same reason (decision 31).
