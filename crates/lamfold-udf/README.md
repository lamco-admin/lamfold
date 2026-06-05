# lamfold-udf

> The UDF frontend of the [`lamfold`](..) read-only media stack — clean-room
> UDF (ECMA-167 / OSTA 1.02) read.

Reads over a `lamfold::BlockSource` and implements `lamfold::FoldFrontend`, so it
composes through `dispatch_fs_over_source` and co-resides with `lamfold-iso` on
UDF-Bridge optical media. Spec: `the lamfold design spec` §4.

## Status — UDF base

| Capability | State |
| ---------- | ----- |
| Descriptor chain (AVDP → VDS → Partition + Logical Volume → File Set → root ICB) | ✅ done + tested |
| File Entry (tag 261) inodes; File Identifier Descriptors (directories) | ✅ done + tested |
| Data: inline / short_ad / long_ad; UCS-2 + Latin-1 names | ✅ done + tested |
| Extended File Entry (266), metadata/sparable/VAT partition maps (UDF 2.x) | ⏳ deferred |
| extended_ad, allocation-extent continuation, symlink path resolution | ⏳ future |

Verified against a **real** `mkudffs --udfrev=0x0102` image (populated via loop
mount): probe, tree walk, inline small files, a nested directory, and a
long_ad-extent file (5000 bytes, full + mid-extent reads). Builds default +
`--no-default-features` (`no_std`); `#![forbid(unsafe_code)]`.

## Clean-room posture

Derived only from ECMA-167 + OSTA UDF specs and the permissive references in
`NOTICE`. Linux `fs/udf` and `udftools` (GPL) are fenced off — never copied.

## License

MIT OR Apache-2.0.
