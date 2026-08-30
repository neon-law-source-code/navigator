---
publish: true
---

# Marketing copy

The firm's public home and practice pages publish English only. The words live in
`neon/locales/en/*.yaml`. Editing them is a YAML change: the Rust loaders interpolate `{site_name}` and
`{firm_email}`, fill runtime fields a catalog cannot know (hero asset URLs, CLI download archives, `mailto:` hrefs), and
inject the page types the Dioxus routers already take.

There is no translated surface. A second locale directory is a validate error (`Y002`), not a language switch.

## Catalog files

| File | Page |
| --- | --- |
| `neon/locales/en/home.yaml` | `/` |
| `neon/locales/en/litigation.yaml` | `/litigation` |
| `neon/locales/en/fractional-gc.yaml` | `/fractional-gc` |
| `neon/locales/en/fractional-cto.yaml` | `/fractional-cto` |
| `neon/locales/en/navigator.yaml` | `/navigator` |
| `neon/locales/en/services.yaml` | `/services` |

`views::locales` is the typed schema. `navigator validate` deserializes each file as the page its stem names, so a
missing field or an unknown stem fails the gate before a brand crate can load it. The advertising guards in
`neon::firm_copy` still read the loaded pages when the Rust suite runs.

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
