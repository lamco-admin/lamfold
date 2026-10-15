#!/bin/bash
# Build the xorriso fixtures added in lamfold-iso 0.1.1 (the older fixtures were
# made by hand; see each test's header). Needs xorriso and mkfs.vfat.
#   rr_links.iso      Rock Ridge: three empty files and four symlinks, which
#                     xorriso gives the same extent LBA; a 250-character name
#                     and a long link target, which overflow into continuation
#                     areas; an absolute link; a subdirectory.
#   et_appended.iso   A 64 KiB FAT image appended as partition 2 and entered as
#                     the UEFI boot image the way archiso does it. xorriso
#                     records its 128 sectors; archiso's image is larger than
#                     the catalog's 16-bit count holds, so Arch's ISO records 0,
#                     and the test zeroes the count to match.
set -euo pipefail
export PATH="/usr/sbin:/sbin:$PATH"
cd "$(dirname "$0")"
FX="$PWD"
WORK=$(mktemp -d "${TMPDIR:-/var/tmp}/lamfold-iso.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
T="$WORK/tree"
mkdir -p "$T/sub"
printf 'lamfold iso rock ridge fixture\n' > "$T/a.txt"
printf 'inside sub\n' > "$T/sub/inner.txt"
: > "$T/empty1"; : > "$T/empty2"; : > "$T/empty3"
ln -s a.txt "$T/link-rel"
ln -s /usr/bin/bash "$T/link-abs"
ln -s sub "$T/link-dir"
long="$(printf 'component-%02d/' $(seq 1 25))target"
ln -s "$long" "$T/link-long"
name250="$(printf 'n%.0s' $(seq 1 246)).txt"
printf 'long name\n' > "$T/$name250"
rm -f rr_links.iso
xorriso -as mkisofs -quiet -R -o "$FX/rr_links.iso" "$T"
printf '%s\n' "$long" > rr_links.longlink
printf '%s\n' "$name250" > rr_links.longname

E="$WORK/et"
mkdir -p "$E"
printf 'appended efi fixture\n' > "$E/readme.txt"
mkfs.vfat -C -n LAMFOLDEFI "$WORK/efi.img" 64 > /dev/null
rm -f et_appended.iso
xorriso -as mkisofs -quiet -R -o "$FX/et_appended.iso" \
  -append_partition 2 0xef "$WORK/efi.img" \
  -e --interval:appended_partition_2:all:: -no-emul-boot "$E"
ls -l rr_links.iso et_appended.iso | awk '{print "  " $9 " " $5 " bytes"}'
