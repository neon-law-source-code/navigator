#!/bin/sh
# ENG-671: the missing Linux install path for a machine that is not a GitHub
# Actions runner — a developer box, a bare container, a Cloud Build step.
# macOS already has the Homebrew tap; this covers Linux with curl and tar.
#
#   curl -fsSL https://github.com/neon-law-source-code/navigator/releases/download/26.9.13/navigator-26.9.13-install.sh \
#       | sh -s -- --version 26.9.13
#
# `deploy.yml` attaches this script to every release as `navigator-<tag>-install.sh`,
# tag-exact like every other archive it publishes — the script's own logic
# can change release to release, so a reader pinned to one release gets the
# installer that shipped with it.
#
# `--version` is optional: with none given, this reads `version:` out of a
# `navigator.yaml` in the current directory, the same default
# `.github/actions/navigator-install` uses. Either way the tag must be exact
# — never `latest`, `main`, `HEAD`, or empty; a rolling pointer cannot be
# reproduced. Installs to `$HOME/.local/bin` by default, so no `sudo` is
# needed; override with `--dir` or `NAVIGATOR_INSTALL_DIR`.

set -eu

repo="${NAVIGATOR_INSTALL_REPO:-neon-law-source-code/navigator}"
install_dir="${NAVIGATOR_INSTALL_DIR:-${HOME:-/tmp}/.local/bin}"
version=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            version="$2"
            shift 2
            ;;
        --version=*)
            version="${1#--version=}"
            shift
            ;;
        --dir)
            install_dir="$2"
            shift 2
            ;;
        --dir=*)
            install_dir="${1#--dir=}"
            shift
            ;;
        *)
            echo "navigator-install: unrecognized argument '$1'" >&2
            exit 1
            ;;
    esac
done

if [ -z "${version}" ] && [ -f navigator.yaml ]; then
    version="$(grep -E '^version:' navigator.yaml | head -n1 | \
        sed -E 's/^version:[[:space:]]*//; s/[[:space:]]*#.*$//; s/^"//; s/"$//' | \
        sed -E "s/^'//; s/'\$//")"
fi

case "${version}" in
    latest | main | HEAD | "")
        echo "navigator-install: version must be an exact release tag, not '${version}'." >&2
        exit 1
        ;;
esac

case "$(uname -s)" in
    Linux) ;;
    Darwin)
        echo "navigator-install: this script installs the Linux CLI only — on macOS, run 'brew install neon-law-source-code/navigator/navigator'." >&2
        exit 1
        ;;
    *)
        echo "navigator-install: unsupported platform '$(uname -s)' — this script installs the Linux CLI only." >&2
        exit 1
        ;;
esac

asset="navigator-${version}-linux.tar.gz"
checksum="${asset}.sha256"
# Overridable so a test can point this at a local server instead of
# github.com; unset in ordinary use.
base_url="${NAVIGATOR_INSTALL_BASE_URL:-https://github.com/${repo}/releases/download/${version}}"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

echo "Downloading ${asset} from ${repo}@${version}" >&2
curl -fsSL "${base_url}/${asset}" -o "${tmp}/${asset}"

# The checksum sidecar is a newer addition than some already-published
# releases, so its absence is a warning, not a failure. A sidecar that
# downloads but does not match the archive is a real integrity failure and
# still stops the install.
if curl -fsSL "${base_url}/${checksum}" -o "${tmp}/${checksum}" 2>/dev/null; then
    (cd "${tmp}" && sha256sum -c "${checksum}")
else
    echo "navigator-install: ${repo}@${version} publishes no ${checksum} — skipping checksum verification." >&2
fi

tar -xzf "${tmp}/${asset}" -C "${tmp}" navigator

mkdir -p "${install_dir}"
cp "${tmp}/navigator" "${install_dir}/navigator"
chmod 0755 "${install_dir}/navigator"

echo "navigator ${version} installed to ${install_dir}/navigator" >&2
case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
        echo "Add it to PATH: export PATH=\"${install_dir}:\${PATH}\"" >&2
        ;;
esac
