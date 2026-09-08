# Build Runbook: CLI Binaries Locally (cross-compile)

**Scope:** build the `zeroclaw` CLI binary **on your own machine** (no GitHub
Actions), primarily to produce a Linux arm64 executable for on-device testing.

This is the hands-on counterpart to [`build-cli-binaries.md`](./build-cli-binaries.md),
which documents the **CI release pipeline** (tag-triggered, publishes GitHub
Release assets). Use this runbook when you want a throwaway binary to validate
a change on real hardware — a Raspberry Pi, a Graviton box — before cutting a
release. It does not publish anything.

## Why Docker on a Windows host

The Linux binaries dynamically link against **glibc**. A native Windows
(MSVC) or macOS toolchain cannot emit a glibc ELF, so the arm64/x64 Linux
builds are produced inside a **Linux container** that matches what CI uses.

This runbook was validated on `windows/amd64` with Docker Desktop, but the
container recipe is identical on macOS and Linux hosts.

## Prerequisites

| Requirement | Notes |
|---|---|
| Docker (Desktop or Engine) | The build runs in a container; verify with `docker info`. |
| A `rust` base image | `docker pull rust:1-bookworm`. Bookworm's glibc (2.36) yields a `GLIBC_2.34` symbol floor — see below. |
| Node.js on the host | Only for the web-asset step. Pin to `.nvmrc` (currently `24`). |

The cross toolchain (`gcc-aarch64-linux-gnu`) and the Rust target are
installed *inside* the container by the build command, so you do not need them
on the host.

## Step 1 — Build the web dashboard first

The gateway **embeds `web/dist` at compile time**. Build the dashboard assets
*before* the Rust build, or the binary ships with a stale (or missing)
dashboard. Use the xtask wrapper — **not** `cd web && npm run build`, which
skips OpenAPI spec generation and `openapi-typescript`:

```bash
cargo web build
```

This renders `target/openapi.json`, regenerates
`web/src/lib/api-generated.ts`, and emits `web/dist/`. Re-run it whenever
`web/` has changed since your last build; skip it for pure Rust-only changes.

## Step 2 — Cross-compile in the container

The linker and target come from `.cargo/config.toml`
(`[target.aarch64-unknown-linux-gnu] linker = "aarch64-linux-gnu-gcc"`).
Mirror CI: install the Debian cross compiler and export the linker env.

```bash
MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' \
docker run --rm -v "$PWD":/build -w /build \
  -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  rust:1-bookworm bash -c '
    set -e
    apt-get update -qq
    apt-get install -y -qq gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
    rustup target add aarch64-unknown-linux-gnu
    cargo build --release --locked --target aarch64-unknown-linux-gnu --bin zeroclaw
  '
```

Output: `target/aarch64-unknown-linux-gnu/release/zeroclaw`.

> **Git Bash on Windows:** `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'` is
> required. Without it MSYS rewrites the container path `-w /build` into a
> host path like `C:/Program Files/Git/build`, and the container fails to
> start with `the working directory ... is invalid`. In `cmd`/PowerShell the
> vars are unnecessary.

For other targets, swap the target and drop the cross-compiler bits:

| Target | Extra flags |
|---|---|
| `x86_64-unknown-linux-gnu` | no cross compiler; `-e` linker env not needed |
| `aarch64-unknown-linux-gnu` | as above (Debian cross gcc) |

A first full build takes ~10 min; incremental rebuilds (single-file Rust
changes) reuse `target/` and are much faster.

## Step 3 — Verify the artifact

```bash
file target/aarch64-unknown-linux-gnu/release/zeroclaw
# expect: ELF 64-bit LSB pie executable, ARM aarch64, ... interpreter /lib/ld-linux-aarch64.so.1

# glibc symbol floor (run in a Linux container that ships binutils)
docker run --rm -v "$PWD":/build -w /build rust:1-bookworm \
  readelf -V target/aarch64-unknown-linux-gnu/release/zeroclaw \
  | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1
# expect: GLIBC_2.34
```

`GLIBC_2.34` is satisfied by Raspberry Pi OS (glibc 2.36+) and Ubuntu 24.04
(2.39). If a target host reports `GLIBC_x.y not found`, its glibc is older
than this floor — lower the base image's glibc, do **not** raise it.

Record the checksum for the deploy side:

```bash
sha256sum target/aarch64-unknown-linux-gnu/release/zeroclaw
```

## Deploy to the device

```bash
# copy the binary to the arm64 host, then on the host:
mkdir -p ~/.cargo/bin
cp zeroclaw ~/.cargo/bin/zeroclaw && chmod +x ~/.cargo/bin/zeroclaw
zeroclaw --version
```

Install to wherever `which zeroclaw` resolves — typically `~/.cargo/bin`,
**not** `/usr/local/bin` (PATH order shadows the latter). If the instance is
service-managed, `systemctl stop zeroclaw` before overwriting (`kill` alone
lets systemd restart the old process). A later `cargo install` or on-host
`cargo build` will silently overwrite this binary.

## Differences from the CI pipeline

| | Local (this runbook) | CI (`build-cli-binaries.md`) |
|---|---|---|
| Trigger | manual, on your machine | version tag / Actions |
| Base image | `rust:1-bookworm` (glibc 2.36) | `ubuntu-22.04` (glibc 2.35) |
| Resulting arm64 floor | `GLIBC_2.34` | `GLIBC_2.34` (same, verified) |
| Output | bare binary under `target/` | release assets (`.tar.gz`/`.zip`) |

Both base images yield the same `GLIBC_2.34` floor for the arm64 binary, so a
locally built artifact is compatible with the same devices as the CI release.

## Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| Container: `working directory ... is invalid` | MSYS rewrote `-w /build` on Git Bash | Prefix with `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'` |
| Ship with stale dashboard | `web/dist` not rebuilt after a `web/` change | Run `cargo web build` (Step 1) before the Rust build |
| `cargo web build` fails | OpenAPI / `openapi-typescript` step | Do not substitute `cd web && npm run build` |
| Link error `Relocations in generic ELF (EM: 183)` | wrong-arch C toolchain selected | Confirm `aarch64-linux-gnu-gcc` is installed in the container and the linker env is exported |
| `GLIBC_x.y not found` on target | target glibc older than the floor | Lower the container base image glibc; never raise the runner/image |
| Container writes `target/` as root | Docker ran as root | On Linux hosts add `--user "$(id -u):$(id -g)"`; Docker Desktop on Windows/Mac is unaffected |

## Related

- [`build-cli-binaries.md`](./build-cli-binaries.md) — CI release pipeline (the canonical, published path).
- `docs/book/src/hardware/raspberry-pi-setup.md` — deploying to a Pi.
