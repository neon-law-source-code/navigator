#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the Neon Law Navigator workspace.
#
# Default loop: the zero-infrastructure Rust build / test / lint / CLI path
# (each test opens its own embedded, memory-backed SurrealDB). This script
# also installs Docker CE, kind v0.32.0, and helm so a task that needs the
# KIND dependency tier can run `navigator dev up` after `.cursor/start.sh`
# has started dockerd. Cluster creation stays opt-in — do not start KIND here.
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
#      * Docker CE + fuse-overlayfs + iptables-legacy — nested containers on
#        a Cloud Agent (Cursor's "complex Docker" recipe). Pin matches
#        https://cursor.com/docs/cloud-agent/setup#running-docker.
#      * kind v0.32.0 + helm — the `dev up` tool gate in
#        `cli/src/devx/orchestrate.rs`; kind version is the pin in
#        `.github/workflows/deploy.yml`.
#    Idempotent: apt is a no-op when the packages are current, binaries install
#    only when absent, and the whole step is skipped without passwordless sudo
#    (a base image that already ships these).
if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
  apt_env=(DEBIAN_FRONTEND=noninteractive)
  apt_get=(apt-get -o Dpkg::Options::=--force-confold)
  need_apt=()
  pkg-config --exists openssl 2>/dev/null || need_apt+=(libssl-dev pkg-config)
  command -v ld.lld >/dev/null 2>&1 || need_apt+=(lld)
  command -v fuse-overlayfs >/dev/null 2>&1 || need_apt+=(fuse3 fuse-overlayfs)
  command -v iptables >/dev/null 2>&1 || need_apt+=(iptables)
  if [ "${#need_apt[@]}" -gt 0 ]; then
    sudo apt-get update -qq
    sudo "${apt_env[@]}" "${apt_get[@]}" install -y -qq "${need_apt[@]}"
  fi
  if [ -x /usr/sbin/iptables-legacy ]; then
    sudo update-alternatives --set iptables /usr/sbin/iptables-legacy >/dev/null
    sudo update-alternatives --set ip6tables /usr/sbin/ip6tables-legacy >/dev/null
  fi
  if ! command -v kubectl >/dev/null 2>&1; then
    kver="$(curl -sL https://dl.k8s.io/release/stable.txt)"
    curl -sSLo /tmp/kubectl "https://dl.k8s.io/release/${kver}/bin/linux/amd64/kubectl"
    sudo install -m 0755 /tmp/kubectl /usr/local/bin/kubectl && rm -f /tmp/kubectl
  fi
  if ! command -v docker >/dev/null 2>&1; then
    sudo install -m 0755 -d /etc/apt/keyrings
    if [ ! -f /etc/apt/keyrings/docker.gpg ]; then
      curl --retry 3 --retry-delay 5 -fsSL https://download.docker.com/linux/ubuntu/gpg \
        | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
      sudo chmod a+r /etc/apt/keyrings/docker.gpg
    fi
    if [ ! -f /etc/apt/sources.list.d/docker.list ]; then
      echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" \
        | sudo tee /etc/apt/sources.list.d/docker.list >/dev/null
    fi
    sudo apt-get update -qq
    sudo "${apt_env[@]}" "${apt_get[@]}" install -y -qq \
      docker-ce=5:28.5.2-1~ubuntu.24.04~noble \
      docker-ce-cli=5:28.5.2-1~ubuntu.24.04~noble \
      containerd.io \
      docker-buildx-plugin \
      docker-compose-plugin
  fi
  sudo mkdir -p /etc/docker
  sudo tee /etc/docker/daemon.json >/dev/null <<'EOF'
{
  "storage-driver": "fuse-overlayfs"
}
EOF
  sudo groupadd -f docker
  sudo usermod -aG docker ubuntu
  if ! command -v kind >/dev/null 2>&1 || ! kind version 2>/dev/null | grep -q 'v0.32.0'; then
    curl -fsSLo /tmp/kind "https://kind.sigs.k8s.io/dl/v0.32.0/kind-linux-amd64"
    sudo install -m 0755 /tmp/kind /usr/local/bin/kind
    rm -f /tmp/kind
  fi
  if ! command -v helm >/dev/null 2>&1; then
    curl -fsSL "https://get.helm.sh/helm-v3.17.3-linux-amd64.tar.gz" \
      | tar -xz -C /tmp linux-amd64/helm
    sudo install -m 0755 /tmp/linux-amd64/helm /usr/local/bin/helm
    rm -rf /tmp/linux-amd64
  fi
else
  echo "warning: passwordless sudo unavailable; skipping system package setup (libssl-dev, lld, kubectl, docker, kind, helm)" >&2
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
