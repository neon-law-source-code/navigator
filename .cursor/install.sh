#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the Neon Law Navigator workspace.
#
# Scope: the zero-infrastructure Rust build / test / lint / CLI loop — the
# contributor floor AGENTS.md documents ("The Rust test suite needs no
# database, no container, and no configuration: each test opens its own
# embedded, memory-backed SurrealDB"). The full-stack `neon` web server runs
# against the KIND dependency tier on a developer's own machine; that nested
# Docker + Kubernetes topology is deliberately out of scope for the Cloud
# Agent base environment.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# 0. System packages the workspace test gate needs but that the base image
#    does not ship. Each is required by `cargo nextest run --workspace`, not by
#    `cargo build`:
#      * libssl-dev / pkg-config — the `fantoccini` WebDriver client (a
#        browser-e2e dev-dependency) pulls in `openssl-sys`, which needs the
#        OpenSSL development headers to compile.
#      * lld — the LLVM linker CI links the ~40 test binaries with; several
#        statically link most of the tree, so the default linker is slower and
#        far heavier on RAM (see .github/workflows/ci.yml).
#      * kubectl — `cli::devx::ship` tests render manifests with
#        `kubectl kustomize`; without it those tests fail with ENOENT.
#    Idempotent: apt is a no-op when the packages are current, kubectl installs
#    only when absent, and the whole step is skipped without passwordless sudo
#    (a base image that already ships these).
if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
  need_apt=()
  pkg-config --exists openssl 2>/dev/null || need_apt+=(libssl-dev pkg-config)
  command -v ld.lld >/dev/null 2>&1 || need_apt+=(lld)
  if [ "${#need_apt[@]}" -gt 0 ]; then
    sudo apt-get update -qq
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${need_apt[@]}"
  fi
  if ! command -v kubectl >/dev/null 2>&1; then
    kver="$(curl -sL https://dl.k8s.io/release/stable.txt)"
    curl -sSLo /tmp/kubectl "https://dl.k8s.io/release/${kver}/bin/linux/amd64/kubectl"
    sudo install -m 0755 /tmp/kubectl /usr/local/bin/kubectl && rm -f /tmp/kubectl
  fi
else
  echo "warning: passwordless sudo unavailable; skipping system package setup (libssl-dev, lld, kubectl)" >&2
fi

# 1. Materialize the pinned toolchain. rust-toolchain.toml selects the channel
#    plus the rustfmt and clippy components; invoking rustup here makes that
#    install explicit and fails loudly instead of on the first cargo call.
rustup show active-toolchain >/dev/null 2>&1 || rustup toolchain install
rustc --version
cargo --version

# 2. cargo-nextest — the workspace test runner named by the AGENTS.md
#    verification gate (`cargo nextest run --workspace`). The maintained
#    prebuilt binary drops straight into CARGO_HOME; skip when already present.
if ! cargo nextest --version >/dev/null 2>&1; then
  curl -LsSf https://get.nexte.st/latest/linux \
    | tar zxf - -C "${CARGO_HOME:-$HOME/.cargo}/bin"
fi
cargo nextest --version

# 3. Warm the dependency graph and compile the workspace so a booting agent
#    starts on a ready target cache. Idempotent: a second run is up-to-date.
cargo fetch --locked
cargo build --workspace
