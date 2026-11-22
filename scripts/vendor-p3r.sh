#!/usr/bin/env bash
# Builds vendor/plonky3-recursion: the pinned Plonky3-recursion commit plus the local patches
# vendor/p3r-*.patch (owner-approved 2026-11-22; see vendor/README.md). The directory is
# gitignored and rebuilt whenever the rev or a patch changes. Idempotent.
set -euo pipefail
cd "$(dirname "$0")/.."
REV=3a69ba61f713c8400b14eb3d8f0e52caa13512c7
DST=vendor/plonky3-recursion
STAMP="$REV $(cat vendor/p3r-*.patch | sha256sum | cut -c1-16)"
if [ -f "$DST/.umis-stamp" ] && [ "$(cat "$DST/.umis-stamp")" = "$STAMP" ]; then
  exit 0
fi
src=$(ls -d "$HOME"/.cargo/git/checkouts/plonky3-recursion-*/"${REV:0:7}" 2>/dev/null | head -1 || true)
tmp=""
if [ -z "$src" ]; then
  tmp=$(mktemp -d)
  git clone --quiet https://github.com/Plonky3/Plonky3-recursion "$tmp/p3r"
  git -C "$tmp/p3r" checkout --quiet "$REV"
  src="$tmp/p3r"
fi
rm -rf "$DST"
mkdir -p "$DST"
(cd "$src" && tar --exclude=.git -cf - .) | (cd "$DST" && tar -xf -)
for p in vendor/p3r-*.patch; do
  patch --quiet -d "$DST" -p1 < "$p"
done
echo "$STAMP" > "$DST/.umis-stamp"
[ -n "$tmp" ] && rm -rf "$tmp"
echo "vendor-p3r: $DST at ${REV:0:7} with $(ls vendor/p3r-*.patch | wc -l) patch(es)"
