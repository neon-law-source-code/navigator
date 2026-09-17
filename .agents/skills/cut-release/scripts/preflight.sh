#!/usr/bin/env bash
# Every release check that can fail on this machine, run before the bump is
# pushed.
#
#   preflight.sh [remote]
#
# IT TAKES NO VERSION, and that is the change. The version is whatever
# `[workspace.package].version` says: `ops release version` wrote it, this script
# checks it, `ci.yml` checks it again on the pull request, and `deploy.yml` reads
# it on merge. One value, read in four places, named in one.
#
# Read-only and safely repeatable. It writes nothing, so a failed run costs a
# rerun.
#
# WHAT IT NO LONGER DOES, because the pipeline stopped asking:
#
#   - validate a `YY.M.D` shape or its date. The shape is semver's, checked by
#     `ops release check`; the calendar is a convention with no enforcement.
#   - prove the name is unspent on the remote. `ops release check` compares the
#     version against every release tag, which is the same question asked better.
#   - prove HEAD is reachable from `origin/main`. A merge to `main` is what
#     publishes, so the release source cannot be anything else.
set -euo pipefail

remote="${1:-origin}"

echo "==> fetching ${remote}"
git fetch "${remote}" --tags --prune

echo "==> the working tree must be clean"
if [ -n "$(git status --porcelain)" ]; then
    echo "FAIL: uncommitted changes; a release names a commit, not a desk." >&2
    git status --short >&2
    exit 1
fi
echo "    ok"

# The release decision itself, run locally exactly as `ci.yml` and `deploy.yml`
# run it. Three outcomes: this version is a release, it is already released
# (nothing to publish — you have not bumped yet), or it is BEHIND one already
# published, which fails.
echo "==> is the workspace version a release?"
cargo run -p cli --quiet -- ops release check --no-fetch

echo "==> notices must travel with the distributed binary"
# `cargo fetch` first, and it is load-bearing. `ops notices` reads licence text
# from $CARGO_HOME/registry/src, where cargo unpacks a crate only when something
# needs it — a build unpacks the platform it built for, so a desk that has only
# ever built for macOS has never unpacked the Linux- or Windows-only crates
# Cargo.lock also names. `cargo fetch` with no --target unpacks every target's
# graph, which is what makes the generated file the same on any machine. Without
# it the command refuses, because rendering a partial registry would publish
# this desk's gap as the crates' own.
cargo fetch --locked
cargo run -p cli --quiet -- ops notices --check

# `deploy.yml` builds the release with `--locked` in four places, and `--locked`
# refuses a lock the manifest has moved past. `ops release version` refreshes
# both files together; this proves it happened.
echo "==> Cargo.lock must agree with the manifest"
if ! cargo metadata --locked --format-version 1 >/dev/null 2>&1; then
    echo "FAIL: Cargo.lock does not match Cargo.toml." >&2
    echo "      Re-run: cargo run -p cli -- ops release version --tag <version>" >&2
    exit 1
fi
echo "    ok"

# The reusable workflows and composite actions name this repository's own
# actions by an ABSOLUTE tag, not by the ref the caller used, so nothing but a
# deliberate edit moves them. A pin left behind ships a gate no consumer can
# run: 26.9.17-rc.1 published `project-gate.yml` still pointing
# `navigator-install` at 26.9.16, a tag predating the action, and every job
# needing the CLI died at action resolution.
#
# The rule itself lives in `cli/src/release_pins.rs`, and `ci.yml` runs this
# same command on every pull request. That is the point: a stale pin is a red
# pull request, days before a release cut would have been the first to see it,
# and neither path can hold its own idea of what a stale pin is.
echo "==> the self-referencing action pins must name this version"
cargo run -p cli --quiet -- ops release pins

echo "==> the workspace gate"
cargo nextest run --workspace
cargo test -p features

echo
echo "preflight passed."
echo
echo "THE BROWSER SUITE IS NOT IN THIS GATE, and nothing else runs it before the"
echo "release does. \`ci\` proves the Rust workspace and says nothing about the"
echo "browser and accessibility suites — they self-skip with no harness, so the"
echo "only thing that runs them is deploy.yml's integration job, on the merge"
echo "that publishes. Prove it here instead:"
echo
echo "    cargo run -p cli -- dev browser-e2e"
