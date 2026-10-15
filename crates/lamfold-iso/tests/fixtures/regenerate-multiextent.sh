#!/usr/bin/env bash
# Regenerate the multi-extent fixtures for tests/iso_multiextent.rs.
#
# ECMA-119 records a file of 4 GiB or more as consecutive directory records
# with the same name, each naming its own extent, all but the last flagged
# Multi-Extent (0x80). xorriso splits only at 4 GiB, so a real one can't be a
# small fixture. Instead xorriso writes a plain ISO level 3 image with
# BIG.BIN (10240 bytes) and MARK.TXT, and the file's one directory record is
# rewritten as three records over the same data:
#
#   multiextent.iso       BIG.BIN = extents of 4096, 4096 and 2048 bytes.
#   multiextent-huge.iso  BIG.BIN = two extents of 4294965248 bytes past the
#                         end of the image, then MARK.TXT's extent: 8 GiB plus
#                         the marker. The test reads it through a source that
#                         serves zeros past the image's end.
#
# Requires: xorriso, python3. Usage: ./regenerate-multiextent.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d /var/tmp/lamfold-iso.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

mkdir "$WORK/src"
python3 -c "import sys; sys.stdout.buffer.write(bytes((i * 13 + i // 509) & 0xff for i in range(10240)))" > "$WORK/src/BIG.BIN"
printf 'multi-extent marker\n' > "$WORK/src/MARK.TXT"
touch -d @1700000000 "$WORK/src"/*
SOURCE_DATE_EPOCH=1700000000 xorriso -as mkisofs -quiet -iso-level 3 -V MULTI \
  -o "$WORK/base.iso" "$WORK/src"

python3 - "$WORK/base.iso" "$HERE" <<'PY'
import struct, sys
base, out = sys.argv[1], sys.argv[2]
img = bytearray(open(base, "rb").read())
SECTOR = 2048
pvd = img[16 * SECTOR:17 * SECTOR]
root_lba = struct.unpack_from("<I", pvd, 156 + 2)[0]
root_len = struct.unpack_from("<I", pvd, 156 + 10)[0]

def records(dirbuf):
    pos = 0
    while pos < len(dirbuf) and dirbuf[pos]:
        n = dirbuf[pos]
        yield pos, bytes(dirbuf[pos:pos + n])
        pos += n

def record(template, lba, size, flags):
    r = bytearray(template)
    struct.pack_into("<I", r, 2, lba)
    struct.pack_into(">I", r, 6, lba)
    struct.pack_into("<I", r, 10, size)
    struct.pack_into(">I", r, 14, size)
    r[25] = flags
    return bytes(r)

def rewrite(extents, name):
    d = bytearray(img[root_lba * SECTOR:root_lba * SECTOR + root_len])
    recs = list(records(d))
    big = next(r for _, r in recs if r[33:33 + r[32]] == b"BIG.BIN;1")
    mark = next(r for _, r in recs if r[33:33 + r[32]] == b"MARK.TXT;1")
    ext = [(struct.unpack_from("<I", big, 2)[0], struct.unpack_from("<I", mark, 2)[0])]
    built = []
    for i, (lba, size) in enumerate(extents(*ext[0])):
        last = i == len(extents(*ext[0])) - 1
        built.append(record(big, lba, size, 0 if last else 0x80))
    new = b"".join(r if r != big else b"".join(built) for _, r in recs)
    assert len(new) <= root_len
    d[:] = new + bytes(root_len - len(new))
    out_img = bytearray(img)
    out_img[root_lba * SECTOR:root_lba * SECTOR + root_len] = d
    open(f"{out}/{name}", "wb").write(out_img)

rewrite(lambda big, mark: [(big, 4096), (big + 2, 4096), (big + 4, 2048)], "multiextent.iso")
far = len(img) // SECTOR + 16
rewrite(lambda big, mark: [(far, 4294965248), (far, 4294965248), (mark, 20)], "multiextent-huge.iso")
PY
ls -l "$HERE"/multiextent*.iso
