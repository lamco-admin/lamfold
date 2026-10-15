#!/usr/bin/env bash
# Regenerate the chained-initramfs fixtures for tests/cpio_chain.rs.
#
# An initramfs is often several cpio archives back to back: an uncompressed
# early archive (CPU microcode) and then the main archive, usually compressed.
# The kernel unpacks every one. GNU cpio writes them here, as initramfs-tools
# and dracut do, and pads each to a 512-byte block.
#
#   chain_plain.cpio  early (newc) + main (crc), both uncompressed. The main
#                     archive has a symlink, an empty directory and three hard
#                     links to one file, whose data newc stores once, on the
#                     last link.
#   chain_zstd.cpio   the same early archive + the main archive compressed
#                     with zstd, which lamfold-cpio refuses.
#
# Requires: GNU cpio, zstd. Usage: ./regenerate.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d /var/tmp/lamfold-cpio.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/early/kernel/x86/microcode"
printf 'microcode blob for the fixture\n' > "$WORK/early/kernel/x86/microcode/GenuineIntel.bin"

mkdir -p "$WORK/main/bin" "$WORK/main/etc" "$WORK/main/lib" "$WORK/main/run"
printf '#!/bin/sh\necho init\n' > "$WORK/main/init"
printf 'busybox stand-in\n' > "$WORK/main/bin/busybox"
ln -s busybox "$WORK/main/bin/sh"
printf 'NAME="lamfold-cpio fixture"\n' > "$WORK/main/etc/os-release"
printf 'one file, three names\n' > "$WORK/main/lib/a.so"
ln "$WORK/main/lib/a.so" "$WORK/main/lib/b.so"
ln "$WORK/main/lib/a.so" "$WORK/main/lib/c.so"
chmod 0755 "$WORK/main/init"
find "$WORK/early" "$WORK/main" -exec touch -h -d @1700000000 {} +

archive() { (cd "$1" && find . -mindepth 1 | LC_ALL=C sort | cpio --quiet -o -H "$2" --reproducible); }

archive "$WORK/early" newc > "$WORK/early.cpio"
archive "$WORK/main" crc > "$WORK/main.cpio"
cat "$WORK/early.cpio" "$WORK/main.cpio" > "$HERE/chain_plain.cpio"
zstd -q -19 -c "$WORK/main.cpio" > "$WORK/main.cpio.zst"
cat "$WORK/early.cpio" "$WORK/main.cpio.zst" > "$HERE/chain_zstd.cpio"
ls -l "$HERE"/chain_*.cpio
