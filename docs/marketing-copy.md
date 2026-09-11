---
publish: true
---

# Marketing copy

The firm's public home, practice, and marketing pages publish English only. The words live in
`neon/locales/en/<brand-key>/*.yaml`, one directory per [`views::brand::BrandKey`](../views/src/brand.rs). Editing them
is a YAML change: the Rust loaders pick the directory for the request's brand, interpolate `{site_name}` and
`{firm_email}`, fill runtime fields a catalog cannot know (hero asset URLs, CLI download archives, `mailto:` hrefs), and
inject the page types the Dioxus routers already take.

There is no translated surface. A second locale directory is a validate error (`Y002`), not a language switch. An
unknown brand-key directory is the same error.

## Catalog files

Neon Law (`neon/`):

| File | Page |
| --- | --- |
| `neon/locales/en/neon/home.yaml` | `/` |
| `neon/locales/en/neon/litigation.yaml` | `/litigation` |
| `neon/locales/en/neon/fractional-gc.yaml` | `/fractional-gc` |
| `neon/locales/en/neon/personal-plan.yaml` | `/personal-plan` |
| `neon/locales/en/neon/navigator.yaml` | `/navigator` |
| `neon/locales/en/neon/services.yaml` | `/services` |

The shared catalog sits beside the brand directories rather than inside one, because it is shared across brands and
across repositories:

| File | Role |
| --- | --- |
| `neon/locales/en/shared.yaml` | Sentences both this repository and `navigator-ux` publish |

DeleteYourData.com (`delete-your-data/`):

| File | Page |
| --- | --- |
| `neon/locales/en/delete-your-data/home.yaml` | `/` |
| `neon/locales/en/delete-your-data/services.yaml` | `/services` |

Lawyer Shook (`lawyer-shook/`):

| File | Page |
| --- | --- |
| `neon/locales/en/lawyer-shook/services.yaml` | `/services` (kept valid; the route is not published on that host) |

Lawyer Shook's `/` is not a catalog page. It is a bare holding notice for Shook Law PLLC, written in Rust
(`neon::firm_pages::lawyer_shook_holding_content`), because the page carries no marketing copy to edit.

Which stems a key ships is [`BrandKey::catalog_pages`](../views/src/brand.rs). DeleteYourData.com answers only those
pages plus `/contact` (addresses, not a YAML stem); Lawyer Shook answers `/` alone. Other firm paths 404 on that host
rather than rendering Neon's words.

`views::locales` is the typed schema. `navigator validate` deserializes each file as the page its stem names, so a
missing field or an unknown stem fails the gate before a brand crate can load it. The advertising guards in
`neon::firm_copy` still read the loaded Neon pages when the Rust suite runs.

## The shared catalog

Some sentences are published by this repository *and* by
[`navigator-ux`](https://github.com/neon-law-source-code/navigator-ux), which renders the same public pages in React.
Those sentences are authored once, in `neon/locales/en/shared.yaml`, and nowhere else.

```yaml
catalog_version: 1
entries:
  litigation.title: Your story deserves to be heard.
brands:
  delete-your-data:
    litigation.title: Ask them to delete it.
```

A page catalog references a key instead of repeating the words:

```yaml
heading:
  text: "{shared:litigation.title}"
```

`{shared:<key>}` resolves before `{site_name}` and `{firm_email}`, in the raw YAML, so a reference works in any string
field without the page schema knowing about it. A shared value may itself carry the two brand placeholders.

[`views::locales::shared`](../views/src/locales/shared.rs) is the contract, and `navigator validate` (`Y002`) enforces
all of it:

- **`catalog_version` equals the version this build authors.** A consumer built for another version refuses the
  document rather than rendering half of it.
- **Every key in `REQUIRED_KEYS` is present.** Required copy has no fallback: a missing headline fails the build
  instead of publishing a gap.
- **A value is one line, non-empty, and carries no double quote.** Values are substituted into quoted YAML scalars.
- **A value's only placeholders are `{site_name}` and `{firm_email}`.** Anything else reaches a reader as a literal
  brace.
- **A brand override names a key the shared defaults define.** A brand cannot smuggle a key into a contract the other
  repository does not know about.

**Fallback.** `SharedCatalog::lookup(brand_key, key)` reads that brand's override under `brands:` and otherwise the
shared default under `entries:`. It never reads a different brand's override, so a brand that has not overridden a
sentence publishes the shared one rather than a sibling brand's wording.

Copy that differs between the two renderers today is deliberately **not** in the shared catalog. This catalog removes
duplication; it does not decide wording. Wording lives with the page-copy reviews.

## Exporting the catalog

`navigator-ux` does not read this repository at runtime and does not track `main`. It vendors a generated artifact
pinned to one immutable Navigator revision:

```bash
cargo run -p cli --example export-marketing-catalog -- \
    --revision "$(git rev-parse HEAD)" \
    --out ../navigator-ux/gallery/content/marketing-catalog.json
```

The exporter is an example rather than a `navigator` subcommand on purpose: it is a build-time producer step, not
something an operator runs against a deployment, so it leaves the shipped binary's surface unchanged.

The artifact is `{ "generator", "integrity", "payload" }`. `payload` carries the catalog version, the entries, the brand
overrides, and a `source` naming the repository, the 40-character commit, and the catalog path. `integrity` is `sha256:`
over the **canonical payload** — compact JSON with every object key sorted — which both Rust and TypeScript can
reproduce byte for byte. That is what lets the consumer prove the file it loaded is the file that was exported, and what
makes a hand-edited artifact fail rather than quietly render different words.

Exporting the same catalog at the same revision produces the same bytes. A branch name is refused as a revision, because
a branch is not a pin.

Coverage: [`cli/tests/marketing_catalog_export.rs`](../cli/tests/marketing_catalog_export.rs) drives the real exporter
for determinism, digest coverage, tamper detection, and the refusals;
[`server/tests/firm_routes.rs`](../server/tests/firm_routes.rs) proves each served page renders the sentence the catalog
authors, and that no page leaks an unresolved placeholder.

A home catalog may carry an optional `provenance` block — the flow a request follows, a ledger illustration, three
tiles, and notes — which `webapp::home` renders as one animated card between the service prose and the practice boxes.
Only `neon/locales/en/delete-your-data/home.yaml` publishes one; a brand that keeps no such record omits the block and
renders no section.

The `practices` list in `neon/locales/en/neon/home.yaml` is the firm's practice catalog. The Neon home page renders
those doors, and workshop slides that expand `{{firm-product-cards}}` render the same list. Do not keep a second copy of
the doors in Rust.

## A copy-only pull request

Change the YAML, then run:

```bash
cargo run -p cli --quiet -- validate .
```

CI always runs that command. It skips `cargo test --workspace` when the PR touches no Rust sources. A schema change
belongs in `views::locales` and is a Rust change.

The catalog is compiled into the brand crate with `include_str!`. A merged YAML edit lands on the next image build that
compiles `neon`.

See [`validate.md`](validate.md) for `Y002` and [`gitops.md`](gitops.md) for the conditional rust job.
