# Public assets

Neon Law Navigator's marketing, presentation, workshop, and blog pages render public images through the shared asset
lane. Responsive photos use `views::assets::responsive_picture`; hand-authored heroes and slide media can be dropped
directly under `server/public/img/<slug>/` as PNGs or JPEGs. The bytes are **never** stored in git. Production serves
them from the Google Cloud Storage origin, while local development and the ephemeral KIND integration image hydrate
`server/public/img/` from that same origin. That keeps the repository small (a clone is code, not megabytes of binaries)
without making the local test harness depend on a runtime GCP mount.

## The four commands

The `navigator ops assets` subcommands form a build → publish → restore → verify loop. For responsive photos, the
`views::assets::GALLERY` manifest and the width set (`WIDTHS = [400, 800, 1200]`) are the single source of truth shared
with the view layer, so adding a photo is a manifest edit plus a JPEG — never a code change. Standalone blog,
illustration, presentation, or workshop slide assets do not go through `assets build`; put the finished PNG or JPEG at
its final `server/public/img/<slug>/<name>` path, then use `assets upload` to publish it.

| Command | Direction | What it does |
| --- | --- | --- |
| `assets build` | JPEGs → `server/public/img/` | Re-encode each manifest photo to AVIF/WebP/JPEG at every width. |
| `assets build --only <slug>` | one JPEG → `server/public/img/<slug>/` | Same, for named slugs only. |
| `assets upload` | `server/public/img/` → bucket | Push recognized images to `gs://<project>-assets/img/<slug>/…`. |
| `assets pull` | bucket → `server/public/img/` | Download the published image files back for local development. |
| `assets fetch-referenced` | origin → `server/public/` | Hydrate content `img/…` refs over public HTTPS (no ADC). |
| `assets stub-referenced` | refs → output root | Write tiny placeholders for content `img/…` paths. |
| `assets verify` | `server/content` refs → chosen origin | Fetch every content `img/…` reference; fail if any 404s. |

`build` and `upload` are the publish path for responsive photos, run by whoever curates the gallery. For a finished PNG
hero, only `upload` is needed. `pull` is the restore path every developer runs. `verify` is the pre-ship guardrail.

When restoring a public photo from a local library, copy it to the ignored final path only after confirming it is
firm-owned or rights-cleared and removing EXIF metadata such as GPS coordinates, device identifiers, and capture times.
The local source stays outside Git; the Markdown reference, filename, and accurate alt text are the durable record.

An unfiltered `build` walks the whole manifest and needs every photo's source JPEG on disk, which is not the shape of
adding one photo. `--only <slug>` (repeatable) narrows the run to the slugs you name. An unknown slug fails and lists
the manifest rather than reporting a successful build of nothing.

```bash
cargo run -p cli -- ops assets build --src ~/photos --only berkeley-bay
```

## Adding an image to a presentation or workshop slide

A bucket-lane slide image is complete only when the same file has two homes:

1. The ignored local source at `server/public/img/<deck-slug>/<filename>`, where the development server can preview it.
2. The object `img/<deck-slug>/<filename>` in every deployment bucket that will render the deck.

When an image generator, clipboard, Notes attachment, or conversion tool produces a temporary file, save the
full-resolution result into that final local path before editing the slide Markdown. A file left only under `/tmp`, in a
clipboard attachment, or in the image-generation result is not the local copy. Use PNG for text, diagrams, and
flat-colour art; use JPEG for photographs. HEIC and other unsupported sources must be converted first.

Reference the bucket key without `/public`:

```markdown
![A concise description of the picture](img/rust-in-peace/example.jpg)
```

Then preview locally, publish to staging, and confirm the exact object before production:

```bash
cargo run -p cli -- ops assets verify --base-url http://localhost:<web-port>/public
cargo run -p cli -- ops assets upload --dir server/public/img --bucket neon-law-stg-assets
gcloud storage ls -L gs://neon-law-stg-assets/img/<deck-slug>/<filename>
```

Production is a separate cloud write, not a consequence of staging. An authorized operator uploads and checks the same
key in the production bucket:

```bash
cargo run -p cli -- ops assets upload --dir server/public/img --bucket <production>-assets
gcloud storage ls -L gs://<production>-assets/img/<deck-slug>/<filename>
```

An agent that cannot perform the production write must report it as pending and provide the exact command; it must not
describe the image as published everywhere. After the deployed origin is reachable, run `assets verify` against that
origin as the browser-level publication check. Because the local directory is ignored, another checkout restores the
cloud copy with `assets pull` or `assets fetch-referenced`.

Exact-key checks must report the same byte length and hashes in staging and production. During a full or image-only
roll, `ops ship` opens the selected `NAVIGATOR_ASSETS_BUCKET` directly and refuses to continue if any embedded
presentation or workshop media key is absent. Restart-only skips that preflight because it changes no content or image
version.

## Publishing one photo to every deployment

Each deployment owns its own assets bucket, so a photo is published once per deployment — there is no shared origin that
all three read. The buckets are `NAVIGATOR_ASSETS_BUCKET` in each `deployments/<name>/config.toml`.

| Deployment | Bucket | Public origin (`NAVIGATOR_ASSET_BASE_URL`) |
| --- | --- | --- |
| `neon-law-stg` | `neon-law-stg-assets` | `https://staging.neonlaw.com/assets` |
| the production deployment | its `<deployment>-assets` | its public host's `/assets` |

The origin is the app's own `/assets/{key}` route, not a raw `storage.googleapis.com` URL, which is why the browser
never leaves the site's origin for an image. Publish to staging, verify, then production:

```bash
cargo run -p cli -- ops assets upload --bucket neon-law-stg-assets
cargo run -p cli -- ops assets upload --bucket <production>-assets
```

## Licensed webfonts

The public asset origin also serves GORP Serif. The WOFF2 bytes are never committed or baked into an image: each
operator supplies the fonts from its own TrashType delivery and uploads the initial Regular and Bold faces under
`fonts/gorp-serif/`. `PageLayout` resolves those URLs through `NAVIGATOR_ASSET_BASE_URL`; without that setting, local
development falls back to `/public/fonts/gorp-serif/` for a licensed operator's ignored local copies.

```bash
cargo run -p cli -- ops assets fonts upload \
  --dir '/path/to/GORP Serif/WOFF'
```

The command targets `NAVIGATOR_ASSETS_BUCKET` (or `--bucket`) and refuses a partial delivery. Navigator code is
copyright Shook Law PLLC; [GORP Serif](https://trashtype.com/fonts/gorp) is font software licensed separately from
TrashType. Follow the [TrashType terms](https://trashtype.com/legal) and keep the local notice in
`server/public/fonts/gorp-serif/LICENSE.txt` with the source files. Before the first upload, rerun `navigator ops gcp
setup` so the public assets bucket receives the font-fetch CORS policy.

The desktop `.otf` family from the same delivery is a _restricted_ download for firm workers who need the installable
faces. It is published separately, as one ZIP — and, crucially, to the **private documents bucket**
(`NAVIGATOR_DOCUMENTS_BUCKET`), not the public assets bucket the WOFF2 web faces use. The web faces are public because
browsers fetch them auth-free; the installable family must stay behind authorization, so it lives where only the gated
route can reach it. Public object URLs cannot bypass embedded Rego. `assets fonts upload-desktop` packages the full
licensed family — every one of the six weights, refusing a partial delivery — into a byte-stable ZIP (sorted,
fixed-timestamp) and uploads it to `fonts/gorp-serif/gorp-serif-otf.zip`:

```bash
NAVIGATOR_DOCUMENTS_BUCKET=<project>-documents cargo run -p cli -- ops assets fonts upload-desktop \
  --dir '/path/to/GORP Serif'
```

The route `GET /app/team/fonts/gorp-serif.zip` streams that object; the `/app/team` home links it as its **Brand fonts**
card. The object rides the team home's own prefix, so embedded Rego's `/app/team` rules admit exactly the four firm
tiers — Owner, Admin, Lawyer, and Clerk — and deny client and anonymous callers. A font ZIP is a firm brand asset, not
lawyer work, so it needs neither the `/app/lawyer` prefix nor the exact-path Clerk exception that prefix used to force.
A missing object is a loud `502`, never a fallback — the same pull-and-verify posture as the vendored government forms.

Publication is not verified by CI. `deploy.yml` builds and publishes images; it never probes a rolled deployment's asset
origin, so an empty bucket reaches production silently. Run `assets verify` against the deployment's own public host
after its roll — that is the only check that looks at what a browser would actually receive:

```bash
cargo run -p cli -- ops assets verify --base-url https://staging.neonlaw.com/assets
cargo run -p cli -- ops assets verify --base-url https://<production>/assets
```

`verify` probes the same key set `orphans` treats as reachable — every markdown `](img/…)` reference, every
`views::assets::GALLERY` variant, and both licensed GORP faces — and exits `2` naming whatever the origin does not
serve.

## Verify before shipping

Because the bytes live only in the bucket, a post can merge and reach a release with a hero that 404s in production —
the rendered-HTML test only checks the `src` string, not that the object exists. `assets verify` closes that gap: it
walks every `img/…` image reference under `server/content`, fetches each one from the public origin (auth-free `HEAD`
against `NAVIGATOR_ASSET_BASE_URL`, exactly as a browser would), and exits non-zero listing any that are missing.

```bash
NAVIGATOR_ASSET_BASE_URL=https://storage.googleapis.com/<project>-assets cargo run -p cli -- ops assets verify
```

Run it after `assets upload`, before you ship. The `deploy` workflow's `build` job runs `assets stub-referenced` before
baking the ephemeral `navigator-web` image used by KIND, writing placeholders under `server/public`; those placeholder
files carry the same paths as the real objects, but not the real photo bytes. After `dev e2e`, the KIND `integration`
job runs `assets verify` against the local host — the serve gate proving the stubbed KIND image serves every referenced
path:

```bash
navigator ops assets verify --base-url http://localhost:8080/public
```

No public origin is probed in CI: publication of the real bytes is the operator upload lane described above, plus the
live site's `/assets` proxy. A release blocks until that local gate passes. Locally, run the same gate against a
host-side server:

```bash
navigator ops assets verify --base-url http://localhost:<web-port>/public
```

### The production origin

In the deployment's `deployments/<name>/config.toml`, set `NAVIGATOR_ASSET_BASE_URL` to the public origin of the assets
bucket — `https://storage.googleapis.com/<project>-assets`, the same bucket as `NAVIGATOR_ASSETS_BUCKET`, with no
`gs://` prefix and no trailing slash. The `img/` prefix comes from the reference path, so the browser fetches
`…/<project>-assets/img/<slug>/<file>`. `navigator ops ship` enforces this: it aborts before any rollout if the value is
unset, so a prod deploy can never roll a `web` that resolves images against an empty same-origin `/public`.

## Why the images aren't in git

`server/public/img/` is ignored by `.gitignore`. A fresh clone has **empty image slots** — the rest of `server/public`
(Bootstrap, brand SVGs, vendored JS/CSS) stays tracked and still ships in the image, but page images do not. The
production app resolves image URLs through `views::assets::asset_url`, which prefixes `NAVIGATOR_ASSET_BASE_URL` (the
bucket's public origin), so browsers fetch images straight from the bucket. The single cross-origin allowance in the
Content-Security-Policy (`img-src`) is exactly this. The CI KIND image is the exception: it bakes temporary placeholders
under `/public` so browser tests exercise the local static-file path without requiring GCP credentials or real photo
bytes.

## CI placeholders and local development

`assets stub-referenced` is for CI image packaging checks. It scans the same `server/content` Markdown references as
`verify`, creates each parent directory under the chosen output root, and writes a minimal valid image file matching the
referenced extension. Run public-origin `verify` first; the stub command does not contact GCS and does not prove
publication. Its job is only to let the local `/public/img/...` route serve something at the exact keys already proven
live in the public bucket.

Because the slots are empty on a fresh clone, the dev `/public` mount 404s every page image until you populate
`server/public/img/`. The fast path is to **pull** the already-published files from the bucket — no source JPEGs, no
re-encode, and no generated-image source needed:

```bash
NAVIGATOR_STORAGE_ENDPOINT= cargo run -p cli -- ops assets pull --bucket neon-law-stg-assets
```

This downloads every supported image file (`.avif`, `.webp`, `.jpg`, `.jpeg`, `.png`) under the bucket's `img/` prefix
into `server/public/img/<slug>/…`, byte-identical to what was uploaded. Run it once after cloning, and again whenever
public page images change; the KIND dev loop then serves the images from `/public` with no further setup. Auth is ADC
(`gcloud auth application-default login`); this operator command targets real GCS.

**Clear `NAVIGATOR_STORAGE_ENDPOINT` on the command line, as above.** Every `navigator` invocation loads `.devx/env`
(`cli/src/main.rs`), and a worktree's `.devx/env` points that variable at the local Garage emulator. A `pull` from a
working KIND checkout therefore asks `localhost` for a GCS bucket and fails `403 Forbidden` on a URL naming the right
bucket, which reads like a permissions problem and is not one. `env -u` does not help: unsetting the variable only lets
`dotenvy` supply it from the file again. Assigning it empty works because the storage layer treats an empty endpoint as
absent.

### No ADC? Pull over public HTTPS instead

The published bytes are world-readable at the deployment's own origin, so a developer without GCP credentials can fetch
them with no auth at all:

```bash
cargo run -p cli -- ops assets fetch-referenced --base-url https://staging.neonlaw.com/assets
```

**This covers content images only, and that difference bites.** `fetch-referenced` scans `server/content` Markdown for
`img/…` references, so it restores blog and workshop images and not the photos the `views::assets::GALLERY` manifest
declares. The firm home page's `berkeley-bay` hero and the team portraits are manifest entries, referenced from Rust
rather than from Markdown, so on a fresh clone with no ADC the blog fills in and the home page's hero stays a broken
image. Until `fetch-referenced` learns the manifest, fetch a manifest photo's variants directly; the widths and formats
are the ones `views::assets` generates:

```bash
mkdir -p server/public/img/berkeley-bay
for w in 400 800 1200; do for ext in avif webp jpg; do
  curl -fsS -o "server/public/img/berkeley-bay/berkeley-bay-${w}w.${ext}" \
    "https://staging.neonlaw.com/assets/img/berkeley-bay/berkeley-bay-${w}w.${ext}"
done; done
```

If you are _curating_ the gallery (adding or replacing a responsive photo), use `build` from the source JPEGs and then
`upload` instead — see [The four commands](#the-four-commands) above. If you are adding a blog hero PNG, put it under
`server/public/img/<slug>/`, verify it locally, then run `assets upload`.
