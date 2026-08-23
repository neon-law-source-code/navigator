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
