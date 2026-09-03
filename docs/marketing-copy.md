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
| `neon/locales/en/neon/fractional-cto.yaml` | `/fractional-cto` |
| `neon/locales/en/neon/navigator.yaml` | `/navigator` |
| `neon/locales/en/neon/services.yaml` | `/services` |

DeleteYourData.com (`delete-your-data/`):

| File | Page |
| --- | --- |
| `neon/locales/en/delete-your-data/home.yaml` | `/` |
| `neon/locales/en/delete-your-data/services.yaml` | `/services` |

Which stems a key ships is [`BrandKey::catalog_pages`](../views/src/brand.rs). A house brand answers only those pages
plus `/contact` (addresses, not a YAML stem); other firm paths 404 on that host rather than rendering Neon's words.

`views::locales` is the typed schema. `navigator validate` deserializes each file as the page its stem names, so a
missing field or an unknown stem fails the gate before a brand crate can load it. The advertising guards in
`neon::firm_copy` still read the loaded Neon pages when the Rust suite runs.

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
