#!/usr/bin/env bash
# Regenerate fixtures/large.sqfs for tests/squash_large.rs.
#
#   huge.bin   600 MiB, zeros except a marker in its last 64 bytes. mksquashfs
#              stores the zero blocks as sparse, so the image stays small, but
#              the file is larger than lamfold's 512 MiB whole-file read cap.
#   multi.bin  300000 bytes of a counting pattern: two full 128 KiB blocks
#              and a fragment tail.
#
# Requires: mksquashfs (squashfs-tools), python3. Usage: ./regenerate-large.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d /var/tmp/lamfold-squash.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

mkdir "$WORK/src"
truncate -s $((600 * 1024 * 1024)) "$WORK/src/huge.bin"
python3 - "$WORK/src" <<'PY'
import sys
root = sys.argv[1]
with open(f"{root}/huge.bin", "r+b") as f:
    f.seek(600 * 1024 * 1024 - 64)
    f.write(b"end of a 600 MiB file".ljust(64, b"."))
with open(f"{root}/multi.bin", "wb") as f:
    f.write(bytes((i * 7 + i // 251) & 0xFF for i in range(300000)))
PY
touch -d @1700000000 "$WORK/src"/*
mksquashfs "$WORK/src" "$HERE/large.sqfs" -comp zstd -b 131072 -noappend -quiet \
  -all-root -mkfs-time 1700000000 -all-time 1700000000
ls -l "$HERE/large.sqfs"
