# The design system

The browser UI is the **Dioxus Components** theme: Rust components in `webapp/src/components/`, styled by
`server/public/css/theme.css`, rendered server-side by `portal` and hydrated by the same-origin wasm bundle.
[`workspace-layout.md`](workspace-layout.md) maps the crates. `/design` is the living gallery — every block on it is the
production component, so the page cannot drift from what ships.

## Summary

- **One component tree, brand-agnostic, two render modes.** The same component renders an anonymous page that ships no
  hydration bundle and drives an authenticated, hydrated one.
- **`webapp::components` is a leaf.** No router, no session, no application state, no data access, no brand colour.
- **Semantic tokens only.** A component emits a class name; the token module decides what `--nav-*` it resolves to.
- **Rust only.** No Node toolchain, no CDN, no Bootstrap, no HTMX, no Alpine. The design system has one exception, and
  it is not part of the component tree: a deployment that names a support-chat inbox loads that vendor's widget from its
  own origin, injected by the render middleware and admitted by a route-scoped CSP. Nothing a component renders may
  reach off-origin.

```bash
cargo run -p cli -- dev worktree-env up --path "$PWD"
set -a; source .devx/env; set +a
cargo run -p neon   # then open /design after signing in
```

## The leaf contract

A navigable component takes an `href` and renders a plain `<a>`. Nothing in the module imports a router, which is what
lets one component library serve a server-only marketing page *and* an interactive application: navigation is injected
at the call site rather than imported into the component.

| Rule | Enforced by |
| --- | --- |
| No `portal`, `server`, `store`, `sea_orm`, or `cloud` import | `webapp::components::leaf_contract` |
| No router import, no `AppState`, no `SessionData` | `webapp::components::leaf_contract` |
| No literal colour — `--nav-*` tokens only | `webapp::components::leaf_contract` |
| A gallery entry for every component | `webapp::design::tests` |

`views::components::code` is the one permitted crossing: `code.rs` calls its pure `syntect` highlighter inside a
`#[server]` body. That is a rendering helper with no request, session, or route in it. Tighten the forbidden list before
widening that exception.

A raw colour value pins one legal identity into shared code, so it is a defect rather than a style choice.

## Brand tokens

One contract, three sets of values. Nothing in `webapp/src/components/` changes to render a different brand.

1. `server/public/css/tokens.css` declares the `--nav-*` layer — ramp, surfaces, text, radius, font — and its
   `prefers-color-scheme: dark` overrides. `theme.css` imports it before shared rules, so a downstream stylesheet can
   override primary, secondary, and font tokens without changing markup or adding a build pipeline.
2. Shared component rules in `server/public/css/theme.css` consume that layer:
   `.nav-card`, `.nav-btn`, `.nav-table`, `.nav-toast`, and the page chrome each read `var(--nav-…)`, never a hex value.
3. A component emits only those class names. It never carries a colour of its own.

The `/design` palette section paints each swatch from its own token, so the gallery shows whichever brand the request
resolved to.

**The inherited trap:** SSR tests check class names, not CSS. A page whose chrome rules were never added to `theme.css`
ships green and renders unstyled. Every migrated surface adds its chrome rules in the same pull request.

## Two render modes

- **Anonymous, server-rendered** — the firm host's public Dioxus pages (`neon::firm_public_dioxus_routers`) mount
  outside the session boundary. Content resolves from the request's resolved brand on the server, so the page is
  readable with no client bundle.
- **Authenticated and hydrated** — the lawyer, admin, and portal clusters mount inside the session boundary. Dioxus
  ships hydration data as inline scripts, which the strict `script-src` blocks; the per-response nonce middleware is the
  fix, never `unsafe-inline`.

## Adding a component

1. One file in `webapp/src/components/`, exported from `components.rs`.
2. Take data and callbacks as props. Navigate through an `href` prop and a plain anchor, never an imported router.
3. Semantic class names only; add the matching rules to `server/public/css/theme.css` in the same pull request.
4. A test beside it rendering the component through `dioxus_ssr` and asserting the aria semantics and theme classes.
   `breadcrumb.rs` is the reference.
5. Render it in `webapp/src/design.rs`; `gallery_source_mentions_every_exported_component` fails the build otherwise.
6. `cargo nextest run -p webapp --lib && cargo nextest run -p portal --lib`.

## Accessibility

The public sites are a legal-services front door:

- **Assert on role and accessible name in tests,** not on colour.
- **Decorative versus meaningful SVG.** An unlabelled `Icon` is `aria-hidden`; a labelled one carries its name in a
  `<title>`.
- **Announce the current location** with `aria-current="page"`, never colour alone.
- **Keep the focus ring.** It is in the `.nav-btn` base rule; dropping it in a variant is a defect.
- **Check contrast in both schemes.** Verify link and muted text against WCAG 2.2 AA in light and dark.
- **Landmarks, one `h1` per page.** The shell supplies `header`, `nav[aria-label]`, and `footer`; the page supplies one
  `main`. A shell rendered as a preview inside another page passes `main_landmark: false` so it does not nest a second
  landmark.

Dioxus SSR wraps text nodes in hydration comments, so a markup assertion matches `>Text<` rather than `class="x">Text`,
and attributes escape `&` as `&#38;`.

## Gates

CI's `cargo test (workspace)` job runs the whole suite plus `clippy --workspace --all-targets -D warnings`. Locally, run
the affected package rather than the workspace sweep.

## Boundaries

- **Generated PDFs use Typst; transactional email uses server-rendered string templates.**
  See [`AGENTS.md`](../AGENTS.md).
- **Table state stays in the URL.** Filtering, sorting, and pagination are server-validated query parameters: `?sort=`
  follows the JSON:API 1.1 contract and an unadvertised key returns `400`; `?page=` is 1-indexed. The `/design` demo
  table is the reference implementation, guarded by `reject_unadvertised_design_sort`.
- **No CDN, ever.** Every asset is same-origin. Swagger UI (`/app/api`) stays vendored rather than rewritten;
  `server/tests/no_cdn_assets.rs` is the build-time backstop.
- **No rich-text editor.** Long-form input is a plain `<textarea>`.
