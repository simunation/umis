#!/usr/bin/env bash
# Builds the locally patched dependencies (owner-approved 2026-11-22/23; see vendor/README.md),
# both gitignored and rebuilt whenever the pin or a patch changes. Idempotent.
#   vendor/plonky3-recursion: the pinned Plonky3-recursion commit + vendor/p3r-*.patch
#   vendor/p3-batch-stark:    p3-batch-stark 0.9.0-rc.2 from crates.io + vendor/p3-batch-stark-*.patch
set -euo pipefail
cd "$(dirname "$0")/.."
REV=3a69ba61f713c8400b14eb3d8f0e52caa13512c7
DST=vendor/plonky3-recursion
STAMP="$REV $(cat vendor/p3r-*.patch | sha256sum | cut -c1-16)"
vendor_batch_stark() {
  local ver=0.9.0-rc.2 dst=vendor/p3-batch-stark
  local stamp="$ver $(cat vendor/p3-batch-stark-*.patch | sha256sum | cut -c1-16)"
  if [ -f "$dst/.umis-stamp" ] && [ "$(cat "$dst/.umis-stamp")" = "$stamp" ]; then
    return 0
  fi
  local src t=""
  src=$(ls -d "$HOME"/.cargo/registry/src/*/p3-batch-stark-$ver 2>/dev/null | head -1 || true)
  if [ -z "$src" ]; then
    t=$(mktemp -d)
    curl -sSfL "https://static.crates.io/crates/p3-batch-stark/p3-batch-stark-$ver.crate" | tar -xz -C "$t"
    src="$t/p3-batch-stark-$ver"
  fi
  rm -rf "$dst"
  mkdir -p "$dst"
  (cd "$src" && tar -cf - .) | (cd "$dst" && tar -xf -)
  for p in vendor/p3-batch-stark-*.patch; do
    patch --quiet -d "$dst" -p1 < "$p"
  done
  echo "$stamp" > "$dst/.umis-stamp"
  [ -n "$t" ] && rm -rf "$t"
  echo "vendor-p3r: $dst $ver with $(ls vendor/p3-batch-stark-*.patch | wc -l) patch(es)"
}

if ! { [ -f "$DST/.umis-stamp" ] && [ "$(cat "$DST/.umis-stamp")" = "$STAMP" ]; }; then
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
fi
vendor_batch_stark
