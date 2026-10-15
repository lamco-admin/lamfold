#!/usr/bin/env bash
# Regenerate fixtures/hardlinks.img for tests/romfs_hardlinks.rs.
#
# genromfs writes the first name of a multiply-linked file as a regular file
# and every other name as a hard link (type 0) whose info field is the first
# name's header: Here b.txt and sub/c.txt link to a.txt.
#
# Requires: genromfs. Usage: ./regenerate-hardlinks.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d /var/tmp/lamfold-romfs.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/src/sub"
printf 'shared by three names\n' > "$WORK/src/a.txt"
ln "$WORK/src/a.txt" "$WORK/src/b.txt"
ln "$WORK/src/a.txt" "$WORK/src/sub/c.txt"
printf 'plain\n' > "$WORK/src/plain.txt"
ln -s a.txt "$WORK/src/link"
genromfs -f "$HERE/hardlinks.img" -d "$WORK/src" -V hardlinks
ls -l "$HERE/hardlinks.img"
