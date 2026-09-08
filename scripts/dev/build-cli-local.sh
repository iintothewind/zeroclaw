#!/usr/bin/env bash
# Build a deployable zeroclaw CLI locally (cross-compile via Docker) with the
# CURRENT web dashboard baked into the binary.
#
# THIS IS THE CANONICAL LOCAL PATH for on-device testing (e.g. Raspberry Pi).
# Do NOT hand-roll `docker run … cargo build` without --features embedded-web:
# default features do NOT embed web/dist, so replacing only the binary leaves
# the device serving a stale on-disk gateway.web_dist_dir and your WebUI
# changes will appear to "not take effect".
#
# Usage:
#   scripts/dev/build-cli-local.sh
#   scripts/dev/build-cli-local.sh --target aarch64-unknown-linux-gnu
#   scripts/dev/build-cli-local.sh --target x86_64-unknown-linux-gnu
#   scripts/dev/build-cli-local.sh --skip-web   # ONLY when web/dist is already fresh
#
# Pipeline (always, unless --skip-web):
#   1. wipe web/dist (keep .gitkeep)
#   2. cargo web build          → fresh web/dist
#   3. cargo clean -p zeroclaw-gateway  → force include_dir! re-embed
#   4. Docker cross-compile with --features embedded-web
#   5. assert the binary contains the current index-*.js fingerprint
#   6. collect into dist/bin/<target>/zeroclaw
#
# Output:
#   target/<target>/release/zeroclaw
#   dist/bin/<target>/zeroclaw          (via collect-dist.sh)
#
# Docs: docs/maintainers/build-cli-local.md
set -euo pipefail

TARGET="aarch64-unknown-linux-gnu"
SKIP_WEB=0
RUST_IMAGE="${ZEROCLAW_RUST_IMAGE:-rust:1-bookworm}"

while [ $# -gt 0 ]; do
  case "$1" in
    --target)   TARGET="${2:-}"; shift 2 ;;
    --target=*) TARGET="${1#*=}"; shift ;;
    --skip-web) SKIP_WEB=1; shift ;;
    --image)    RUST_IMAGE="${2:-}"; shift 2 ;;
    --image=*)  RUST_IMAGE="${1#*=}"; shift ;;
    -h|--help)
      sed -n '2,30p' "$0"
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

if [ -z "$TARGET" ]; then
  echo "error: --target must not be empty" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

die() { echo "error: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

# Git Bash on Windows often lacks ~/.cargo/bin on PATH even when PowerShell has it.
export PATH="${HOME}/.cargo/bin:${PATH:-}"
if [ -n "${USER:-}" ]; then
  export PATH="/c/Users/${USER}/.cargo/bin:${PATH}"
fi
if [ -n "${USERPROFILE:-}" ]; then
  # USERPROFILE is like C:\Users\foo — map to /c/Users/foo for Git Bash.
  _up="$(cygpath -u "$USERPROFILE" 2>/dev/null || true)"
  if [ -n "$_up" ]; then
    export PATH="${_up}/.cargo/bin:${PATH}"
  fi
fi

need_cmd docker
need_cmd cargo
need_cmd node
need_cmd npm

# ── 1–3. Fresh web/dist + force gateway rebuild ───────────────────────────
if [ "$SKIP_WEB" -eq 0 ]; then
  step "wipe web/dist (keep .gitkeep) so no stale hashed assets survive"
  if [ -d web/dist ]; then
    find web/dist -mindepth 1 ! -name '.gitkeep' -exec rm -rf {} + 2>/dev/null \
      || find web/dist -mindepth 1 ! -name '.gitkeep' -print0 | xargs -0 rm -rf
  fi
  mkdir -p web/dist
  touch web/dist/.gitkeep

  step "cargo web build (OpenAPI + openapi-typescript + vite → web/dist)"
  cargo web build
  [ -f web/dist/index.html ] || die "web/dist/index.html missing after cargo web build"

  step "cargo clean -p zeroclaw-gateway (force include_dir! to re-read web/dist)"
  cargo clean -p zeroclaw-gateway
else
  step "skipping web rebuild (--skip-web); requiring existing web/dist/index.html"
  [ -f web/dist/index.html ] || die "web/dist/index.html missing; refuse --skip-web"
fi

FINGERPRINT="$(grep -oE 'assets/index-[A-Za-z0-9_-]+\.js' web/dist/index.html | head -1 || true)"
[ -n "$FINGERPRINT" ] || die "could not read index-*.js fingerprint from web/dist/index.html"
echo "    web fingerprint: $FINGERPRINT"

# ── 4. Cross-compile inside Docker with embedded-web ──────────────────────
# default features do NOT include embedded-web. Passing it explicitly is what
# makes include_dir!("…/web/dist") compile into the binary.
FEATURES="embedded-web"

DOCKER_ENV=(-e "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc")
APT_PKGS="gcc-aarch64-linux-gnu g++-aarch64-linux-gnu"
case "$TARGET" in
  aarch64-unknown-linux-gnu)
    ;;
  x86_64-unknown-linux-gnu)
    DOCKER_ENV=()
    APT_PKGS=""
    ;;
  *)
    die "unsupported --target '$TARGET' (supported: aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu)"
    ;;
esac

step "docker cross-compile ($TARGET, features=$FEATURES) via $RUST_IMAGE"

# Git Bash on Windows rewrites /build unless path conversion is disabled.
export MSYS_NO_PATHCONV=1
export MSYS2_ARG_CONV_EXCL='*'

# shellcheck disable=SC2086
docker run --rm \
  -v "$REPO_ROOT:/build" \
  -w /build \
  "${DOCKER_ENV[@]}" \
  "$RUST_IMAGE" \
  bash -c "
    set -e
    apt-get update -qq
    if [ -n \"$APT_PKGS\" ]; then
      apt-get install -y -qq $APT_PKGS
    fi
    rustup target add \"$TARGET\"
    # embedded-web asserts web/dist/index.html exists at build-script time.
    test -f web/dist/index.html
    cargo build --release --locked \
      --target \"$TARGET\" \
      --features \"$FEATURES\" \
      --bin zeroclaw
  "

BIN="target/$TARGET/release/zeroclaw"
[ -f "$BIN" ] || die "expected binary missing: $BIN"

# ── 5. Prove the CURRENT dashboard is inside the ELF ──────────────────────
step "verify binary embeds current web/dist ($FINGERPRINT)"
bin_has_fingerprint() {
  local bin="$1"
  local fp="$2"
  if command -v strings >/dev/null 2>&1; then
    strings "$bin" | grep -F -q "$fp"
  else
    docker run --rm -v "$REPO_ROOT:/build" -w /build "$RUST_IMAGE" \
      bash -c "strings \"$bin\" | grep -F -q \"$fp\""
  fi
}

if ! bin_has_fingerprint "$BIN" "$FINGERPRINT"; then
  die "binary does NOT contain '$FINGERPRINT' — embedded-web bake failed; refuse to collect a stale/empty dashboard"
fi
echo "    ok: fingerprint present in $BIN"

# ── 6. Collect into dist/bin/<target>/ ────────────────────────────────────
step "collect into dist/bin/$TARGET/"
bash "$REPO_ROOT/scripts/dev/collect-dist.sh" --target "$TARGET" --bin zeroclaw

DIST_BIN="dist/bin/$TARGET/zeroclaw"
[ -f "$DIST_BIN" ] || die "collect-dist did not produce $DIST_BIN"

if command -v sha256sum >/dev/null 2>&1; then
  SUM="$(sha256sum "$DIST_BIN")"
elif command -v shasum >/dev/null 2>&1; then
  SUM="$(shasum -a 256 "$DIST_BIN")"
else
  SUM="$(docker run --rm -v "$REPO_ROOT:/build" -w /build "$RUST_IMAGE" sha256sum "$DIST_BIN")"
fi

echo
echo "DONE"
echo "  binary : $DIST_BIN"
echo "  also   : $BIN"
echo "  web    : $FINGERPRINT (embedded)"
echo "  sha256 : $SUM"
echo
echo "Deploy $DIST_BIN to the device (stop the service first), then hard-refresh the browser."
