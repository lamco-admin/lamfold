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
#   chain_<codec>.cpio  early + main compressed with gzip, zstd, xz (CRC32
#                     check, as the kernel builds it), the legacy .lzma format
#                     or lz4 -l (legacy frames), padded with zeros to a 4-byte
#                     boundary, then a small uncompressed "late" archive, so
#                     the reader has to find where the compressed member ends.
#   chain_bzip2.cpio  early + bzip2 main: Recognised, refused.
#   nested_gzip.cpio  early + gzip(gzip(main)): The kernel doesn't decompress
#                     inside a decompressed member, so this is refused.
#
# Requires: GNU cpio, gzip, zstd, xz, lz4, bzip2. Usage: ./regenerate.sh
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

mkdir -p "$WORK/late/etc"
printf 'written by the late archive\n' > "$WORK/late/etc/late.conf"
find "$WORK/late" -exec touch -h -d @1700000000 {} +

archive "$WORK/early" newc > "$WORK/early.cpio"
archive "$WORK/main" crc > "$WORK/main.cpio"
archive "$WORK/late" newc > "$WORK/late.cpio"
cat "$WORK/early.cpio" "$WORK/main.cpio" > "$HERE/chain_plain.cpio"

# compressed <name> <command...>: early + the compressed main archive, zero
# padded to a 4-byte boundary, + the late archive.
compressed() {
  local name="$1"; shift
  "$@" < "$WORK/main.cpio" > "$WORK/main.$name"
  truncate -s %4 "$WORK/main.$name"
  cat "$WORK/early.cpio" "$WORK/main.$name" "$WORK/late.cpio" > "$HERE/chain_$name.cpio"
}
compressed gzip gzip -n -9 -c
compressed zstd zstd -q -19 -c
compressed xz xz -9 --check=crc32 -c
compressed lzma xz --format=lzma -9 -c
compressed lz4 lz4 -q -l -9 -c

bzip2 -9 -c "$WORK/main.cpio" > "$WORK/main.bz2"
cat "$WORK/early.cpio" "$WORK/main.bz2" > "$HERE/chain_bzip2.cpio"
gzip -n -9 -c "$WORK/main.cpio" | gzip -n -9 -c > "$WORK/main.gz.gz"
cat "$WORK/early.cpio" "$WORK/main.gz.gz" > "$HERE/nested_gzip.cpio"
ls -l "$HERE"/chain_*.cpio "$HERE"/nested_gzip.cpio
