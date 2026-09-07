#!/usr/bin/env bash
# Collect built executables from cargo's target dir into a single dist/ tree.
#
# Usage:
#   scripts/dev/collect-dist.sh                 # host release build -> dist/host/
#   scripts/dev/collect-dist.sh --debug         # use debug instead of release
#   scripts/dev/collect-dist.sh --target aarch64-unknown-linux-gnu
#   scripts/dev/collect-dist.sh --bin zeroclaw --bin zerocode
#
# Output layout (kept under dist/bin/ so it never collides with the
# packaged-release files already tracked in dist/aur, dist/freebsd, ...):
#   dist/bin/host/             (host build)
#   dist/bin/<target-triple>/  (cross build, honors --target)
#
# Cargo keeps building into target/ as usual; this script only copies the
# final binaries, so CI and release scripts that read target/ are unaffected.
set -euo pipefail

PROFILE="release"
TARGET=""
BINS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --debug)    PROFILE="debug"; shift ;;
    --release)  PROFILE="release"; shift ;;
    --target)   TARGET="${2:-}"; shift 2 ;;
    --target=*) TARGET="${1#*=}"; shift ;;
    --bin)      BINS+=("${2:-}"); shift 2 ;;
    --bin=*)    BINS+=("${1#*=}"); shift ;;
    -h|--help)  sed -n '2,16p' "$0"; exit 0 ;;
    *)          echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

if [ -n "$TARGET" ]; then
  SRC="$TARGET_DIR/$TARGET/$PROFILE"
  DEST="$REPO_ROOT/dist/bin/$TARGET"
else
  SRC="$TARGET_DIR/$PROFILE"
  DEST="$REPO_ROOT/dist/bin/host"
fi

if [ ! -d "$SRC" ]; then
  echo "error: build dir not found: $SRC (run cargo build first)" >&2
  exit 1
fi

# Default binaries when none are passed explicitly.
if [ ${#BINS[@]} -eq 0 ]; then
  BINS=(zeroclaw zerocode)
fi

mkdir -p "$DEST"
copied=0
for bin in "${BINS[@]}"; do
  # Match the plain name and any .exe variant (Windows host builds).
  for exe in "$SRC/$bin" "$SRC/$bin.exe"; do
    if [ -f "$exe" ]; then
      cp -f "$exe" "$DEST/$(basename "$exe")"
      echo "  -> ${DEST#$REPO_ROOT/}/$(basename "$exe")"
      copied=$((copied + 1))
      break
    fi
  done
done

if [ "$copied" -eq 0 ]; then
  echo "warning: no binaries found in $SRC for: ${BINS[*]}" >&2
  exit 1
fi

echo "collected $copied binary/binaries into ${DEST#$REPO_ROOT/}"
