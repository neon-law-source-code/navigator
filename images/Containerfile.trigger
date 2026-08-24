# syntax=docker/dockerfile:1.7
#
# Shared trigger image — one thin CronJob entrypoint, parameterized by the
# crate whose scheduled-job binary it ships. Build with `--build-arg CRATE=…`:
#   archives          → starts the `Archives` nightly-export workflow
#   billing-workflows → starts the `BillingCanary` workflow
#   cli               → runs the operational SurrealDB archive
#
# Workflow triggers POST to the Restate ingress to start one workflow
# invocation; the SurrealDB archive instead runs in its own CronJob pod.
# Built as a static musl
# binary; runs on `gcr.io/distroless/cc` because reqwest's TLS needs the
# dynamic loader. The whole workspace is copied (the build context is the
# repo root) so the same Containerfile builds any scheduled-job binary.

FROM rust:1.98-bookworm AS builder

# Which crate's `trigger` binary to build. Required — no sensible default.
ARG CRATE
RUN test -n "$CRATE" \
    || (echo "build-arg CRATE is required (archives|billing-workflows|cli)" && false)

# Which binary within the crate. Defaults to `trigger` (the canary / archives
# entrypoint); billing-workflows also ships `reconcile-trigger`, while the
# SurrealDB archive selects `navigator`.
ARG BIN=trigger

RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# `rust-toolchain.toml` must land BEFORE `rustup target add`: the override
# pins a specific 1.98.0 toolchain that rustup re-syncs the first time cargo
# runs in this dir. Adding the target beforehand (against the base image's
# default toolchain) leaves the re-synced toolchain without the musl std,
# and `cargo build --target …-musl` then fails with `can't find crate for
# std`. Adding it here attaches the target to the toolchain the build uses.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN rustup target add x86_64-unknown-linux-musl
COPY rules             rules
COPY store             store
COPY repos             repos
COPY cli               cli
COPY LICENSE        LICENSE
COPY NOTICE         NOTICE
COPY THIRD-PARTY-NOTICES.txt THIRD-PARTY-NOTICES.txt
COPY portal            portal
COPY server            server
COPY neon              neon
COPY webapp            webapp
COPY views             views
COPY README.md         README.md
COPY telemetry         telemetry
COPY github_webhooks    github_webhooks
COPY forms             forms
COPY workflows         workflows
COPY workflows-service workflows-service
COPY github-runner     github-runner
COPY gateway           gateway
COPY cloud             cloud
COPY live-inquiry      live-inquiry
COPY mcp               mcp
COPY features          features
COPY lsp               lsp
COPY pdf               pdf
COPY templates templates
COPY archives          archives
COPY import            import
COPY billing           billing
COPY billing-workflows billing-workflows
# Every scheduled-job crate (workflows-service, archives, billing-workflows, cli)
# reaches the `views` crate through `workflows`, and
# `views` bakes in `docs/lsp/*.md` with include_str!(concat!(
# CARGO_MANIFEST_DIR, "/../docs/…")). Those files live outside any crate
# dir, so `docs` must be staged explicitly or the builder fails to resolve
# the include paths.
COPY docs              docs
# The `cli` crate (the surreal-archive trigger's CRATE) additionally
# embeds the deployment manifests with include_dir!: `k8s/base`,
# `k8s/components`, `k8s/overlays`, and `examples/deploy/k8s/*` all
# resolve at compile time (deploy run 142339796 failed on the first
# missing directory). Containerfile.runner already stages both trees.
COPY k8s               k8s
COPY examples          examples

RUN cargo build --release --target x86_64-unknown-linux-musl -p "${CRATE}" --bin "${BIN}" \
    && cp "target/x86_64-unknown-linux-musl/release/${BIN}" /trigger-bin

# ---------- Stage 2: runtime ----------

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

WORKDIR /app

COPY --from=builder /trigger-bin /app/trigger

ENV RUST_LOG=info

# Identify the release. The daily `deploy.yml` passes `--build-arg
# RELEASE_TAG=$YY.M.D`; `telemetry::init` reads `NAVIGATOR_RELEASE_TAG`
# and tags every span/metric/log with `service.version`, so each trigger
# run self-reports which release fired it. A local build reports `unknown`.
ARG RELEASE_TAG=unknown
ENV NAVIGATOR_RELEASE_TAG=$RELEASE_TAG

# An image someone pulled is a copy, and its holder has neither the repository
# nor a release archive. BUSL obliges you to display this License conspicuously
# on every copy of the Licensed Work, so the text rides in the runtime layer
# beside the binary — and its parameters are what tell that holder whether their
# own use needs a commercial licence, which they cannot work out from terms they
# were never shown. The label is the registry-native form of the same statement:
# Artifact Registry and GHCR read it for the package page, where a puller
# actually looks.
LABEL org.opencontainers.image.licenses="BUSL-1.1" \
      org.opencontainers.image.vendor="Shook Law PLLC"

COPY LICENSE /app/LICENSE
COPY NOTICE  /app/NOTICE

USER nonroot:nonroot

ENTRYPOINT ["/app/trigger"]
