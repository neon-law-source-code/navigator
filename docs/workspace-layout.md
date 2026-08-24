# Workspace layout

The repository is one Cargo workspace. `Cargo.toml` is the authoritative member list.

- Rust owns the server, rules, notation/templates, workflows, forms, billing, storage, authorization, persistence, and
  CLI. `navigator` owns every machine-bound flow; add no shell scripts or Makefile.
- Rust owns browser presentation too, through Dioxus. There is no Node or pnpm workspace.
- Generated PDFs use Typst and transactional email uses server-rendered string templates.
- `portal` is the mountable Axum application. The `neon` brand crate publishes the public face
  over it; the binary is the site.
- Only `workflows-service` consumes `restate-sdk`; other crates submit through `workflows`.
- `features` uses its custom Cucumber harness and runs with `cargo test -p features`.

## The browser surface

Dioxus renders every page:

- `webapp` holds the Dioxus component tree. `portal` server-side renders it through `dioxus-server`, and
  `navigator dev build-webapp` compiles the same crate to `wasm32-unknown-unknown` for the same-origin client bundle
  that hydrates the server-rendered markup.
- `views` keeps presentation-neutral data and helpers — branding, content loading, notation filling — shared by
  `portal`, the CLI, and the workers.
- `server/public` holds the stylesheets and the few vendored scripts that pages load directly.
- The only first-party TypeScript in the repository is the `lsp/vscode-ext` editor extension.

`neon` serves the firm at `www.neonlaw.com`. The binary serves exactly one face, so no runtime flag can point a
deployment at another entity's public surface.

A brand crate owns that face outright: its marketing copy, its page compositions, and its path table all live in the
crate whose binary publishes them. `portal` owns everything underneath — the authenticated application, the JSON API,
the anonymous protocol ingress, and the Dioxus router constructors a brand composes. The line is the domain:
`cli/tests/brand_crate_dependencies.rs` lets a brand name `portal`, `views`, `webapp`, and `telemetry`, and fails the
build if one reaches for `store`, `workflows`, or the auth machinery. A page that needs a gate wraps itself in
`portal::gated` rather than composing an authorization layer of its own.

`web` binds `3001`. OpenObserve owns the worktree-selected UI port (5080 by default).

## Adding a new crate

A new Cargo member must also enter every affected `images/Containerfile.*` `COPY` list. See
[`durable-workflows.md`](durable-workflows.md) and [`rust-programming.md`](rust-programming.md).

`portal::bootstrap` owns authenticated, protocol, and operational routes; a host supplies public routes as
`Router<AppState>`. Sites share the application and docs but keep public marketing routes disjoint. White-label hosts
use the same seam.
