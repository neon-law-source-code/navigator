# Public assets

Neon Law Navigator's marketing, presentation, workshop, and blog pages render public images through the shared asset
lane. Responsive photos use `views::assets::responsive_picture`; hand-authored heroes and slide media can be dropped
directly under `server/public/img/<slug>/` as PNGs or JPEGs. The bytes are **never** stored in git. The staging
deployment serves them through its public `/assets` route, while local development and the ephemeral KIND integration
image hydrate `server/public/img/` from the local `/public` route. That keeps the repository small (a clone is code, not
megabytes of binaries) without making the local test harness depend on a runtime GCP mount.

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
| `assets verify` | published refs → chosen origin | Fetch every published image and font; fail if any are missing. |

`build` and `upload` are the publish path for responsive photos, run by whoever curates the gallery. For a finished PNG
hero, only `upload` is needed. `pull` is the restore path every developer runs. `verify` is the post-roll guardrail.

When restoring a public photo from a local library, copy it to the ignored final path only after confirming it is
firm-owned or rights-cleared and removing EXIF metadata such as GPS coordinates, device identifiers, and capture times.
The local source stays outside Git; the Markdown reference, filename, and accurate alt text are the durable record.

An unfiltered `build` walks the whole manifest and needs every photo's source JPEG on disk, which is not the shape of
adding one photo. `--only <slug>` (repeatable) narrows the run to the slugs you name. An unknown slug fails and lists
the manifest rather than reporting a successful build of nothing.

```bash
cargo run -p cli -- ops assets build --src ~/photos --only lake-tahoe
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

Then preview locally, publish to staging, and confirm the exact object at the staging origin:

```bash
cargo run -p cli -- ops assets verify --base-url http://localhost:<web-port>/public
cargo run -p cli -- ops assets upload --dir server/public/img --bucket neon-law-stg-assets
gcloud storage ls -L gs://neon-law-stg-assets/img/<deck-slug>/<filename>
```

An agent that cannot perform the staging write must report it as pending and provide the exact command; it must not
describe the image as published. After the staging origin is reachable, run `assets verify` against that origin as the
browser-level publication check. Because the local directory is ignored, another checkout restores the cloud copy with
`assets pull` or `assets fetch-referenced`.

Exact-key checks must report the expected byte length and hashes in staging. During a full or image-only roll, `ops`
`ship` opens the selected `NAVIGATOR_ASSETS_BUCKET` directly and refuses to continue if any embedded presentation or
workshop media key is absent. Restart-only skips that preflight because it changes no content or image version.

## Publishing one photo to every deployment

Each deployment owns its own assets bucket, so a photo is published once per deployment — there is no shared origin that
all three read. The buckets are `NAVIGATOR_ASSETS_BUCKET` in each `deployments/<name>/config.toml`.

| Deployment | Bucket | Public origin (`NAVIGATOR_ASSET_BASE_URL`) |
| --- | --- | --- |
| `neon-law-stg` | `neon-law-stg-assets` | `https://staging.neonlaw.com/assets` |

The origin is the app's own `/assets/{key}` route, not a raw storage URL, so the browser stays on the site's origin.
Publish to staging, then verify the staging origin:

```bash
cargo run -p cli -- ops assets upload --bucket neon-law-stg-assets
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
copyright Neon Law IP LLC; [GORP Serif](https://trashtype.com/fonts/gorp) is font software licensed separately from
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

### DeleteYourData.com's typeface

The `delete-your-data` house brand seeds the closed `plus-jakarta-sans` typeface, matching the brand's entry in the
`navigator-ux` gallery. Plus Jakarta Sans is OFL-1.1, so it carries no `operator_licence_required` flag, but its faces
are bucket-served on the same operator-upload lane as GORP's rather than committed to this repository.

```bash
cargo run -p cli -- ops assets fonts upload --family plus-jakarta-sans \
  --dir '/path/to/plus-jakarta-sans/woff2'
```

The directory must hold `PlusJakartaSans-Regular.woff2` and `PlusJakartaSans-Bold.woff2`; the command uploads both to
`fonts/plus-jakarta-sans/` in the public assets bucket (`--family gorp-serif`, the default, is unchanged). Local
development and tests resolve the same fallback `/public/fonts/plus-jakarta-sans/` path GORP's faces use when
`NAVIGATOR_ASSET_BASE_URL` is unset. `portal::dioxus_app` injects a preload and the `@font-face` block for each brand's
own family. The generated `brand-{key}-tokens.css` declares the same faces alongside the brand's `--nav-font-family`.

### The practice brands' typefaces

The five practice brands wear six more OFL-1.1 families on the same lane. `cli::assets::BUCKET_FONT_FAMILIES` is the one
list all of it reads: `--family` names an entry, `assets verify` probes every entry's two faces, and the orphan scan
spares them. A family joins those three at once rather than one and not the others, which is what went wrong — the five
brands shipped while a hand-written two-entry list stayed at two, so `assets verify` reported a clean origin over six
families it never probed.

| `--family` | Family | Brands |
| --- | --- | --- |
| `gorp-serif` | GORP Serif | Neon Law |
| `plus-jakarta-sans` | Plus Jakarta Sans | DeleteYourData.com |
| `eb-garamond` | EB Garamond | Vesta Estate Planning (display) |
| `source-sans-3` | Source Sans 3 | Vesta, Misericordia Injury Law (body) |
| `source-serif-4` | Source Serif 4 | Misericordia Injury Law (display) |
| `mukta` | Mukta | Abhaya Immigration |
| `public-sans` | Public Sans | DeleteYourDebt.com |
| `libre-franklin` | Libre Franklin | the summons practice |

Each value is spelled as the bucket directory it publishes to, and the directory holds `<Stem>-Regular.woff2` and
`<Stem>-Bold.woff2` — the same `<dir>/<stem>` pair `portal::dioxus_app`'s `bucket_font_head` splits, so the operator
lane and the browser surface derive the object key identically:

```bash
cargo run -p cli -- ops assets fonts upload --family eb-garamond \
  --dir '/path/to/eb-garamond/woff2'
```

We redistribute these bytes from our own buckets, so each family's grant travels with it:
`server/public/fonts/<family>/OFL.txt` carries the upstream notice verbatim, tracked even though the faces are not.
Lawyer Shook's Tinos is the one family whose faces are tracked instead — see below.

The published faces are latin subsets. That is right while the English-only invariant holds, but `views` chose Mukta for
Abhaya specifically for its Devanagari coverage: a Hindi surface would need those faces taken from the upstream release
and converted, not from a latin-subset endpoint.

Publication is not verified by CI. `deploy.yml` builds and publishes images, and its local KIND gate proves only the
placeholder image. A full or image-only `ops ship` run verifies the selected deployment's public asset origin after the
rollout has completed and the worker has re-registered with Restate; a missing or unreachable key fails the command
before it can report the ship complete. Restart-only skips this check because it changes no image or content version.
That post-roll check is the only one that looks at what a browser would actually receive:

```bash
cargo run -p cli -- ops assets verify --base-url https://staging.neonlaw.com/assets
```

`verify` probes the same key set `orphans` treats as reachable — every markdown `](img/…)` reference, every
`views::assets::GALLERY` variant, and both faces of every `BUCKET_FONT_FAMILIES` entry — and exits `2` naming whatever
the origin does not serve.

### Lawyer Shook's Tinos

The `lawyer-shook` house brand uses Tinos under the SIL Open Font License 1.1. The repository carries the Regular and
Bold WOFF2 faces under `server/public/fonts/tinos/`; no raster mark is required because the public header and footer
render the LAWYER SHOOK wordmark as text. `portal::dioxus_app` selects these faces for the Lawyer Shook host. It is the
only brand served from the tracked tree rather than the bucket, so it is the one family absent from
`BUCKET_FONT_FAMILIES`; `published_font_families_cover_every_bucket_face_the_site_emits` accepts either lane and refuses
a face delivered by neither. That guard reads both emitters — `portal::dioxus_app`'s per-brand head fragment and
`views::brand_presentation`'s typeface catalog, which is what a runtime brand picks from in `/app/brands` — so a face
cannot reach a browser through either door without a way to publish it.

## Verify after shipping

A live deployment can serve a 404 hero when the bucket is missing bytes — the rendered-HTML test only checks the `src`
string, not that the object exists. `assets verify` closes that gap: it walks image refs under `server/content`, every
responsive gallery variant and both faces of all eight bucket-served webfont families, then fetches each one from the
public origin (auth-free `HEAD` against `NAVIGATOR_ASSET_BASE_URL`, exactly as a browser would). It exits non-zero
listing whatever the origin does not serve. `ops ship` invokes the same verifier after a full or image-only roll. From a
deploy-only tree — a `--deployments-dir` checkout that carries `deployments/` and no `server/content` — it probes the
same origin for the references the binary embeds instead: the workshop markdown, every gallery variant, and every font
family. It says so on stderr, because the blog's references are the one set that lane cannot see; run `assets verify`
from a source checkout to cover them.

```bash
NAVIGATOR_ASSET_BASE_URL=https://staging.neonlaw.com/assets cargo run -p cli -- ops assets verify
```

Run it after `assets upload`, or let `ops ship` run it after the rollout. The `deploy` workflow's `build` job runs
`assets stub-referenced` before baking the ephemeral `navigator-web` image used by KIND, writing placeholders under
`server/public`; those placeholder files carry the same paths as the real objects, but not the real photo bytes. After
`dev e2e`, the KIND `integration` job runs `assets verify` against the local host — the serve gate proving the stubbed
KIND image serves every referenced path:

```bash
navigator ops assets verify --base-url http://localhost:8080/public
```

CI does not probe a public origin: publication of the real bytes is the operator upload lane described above. The live
site's `/assets` proxy is checked after a roll, and `ops ship` blocks completion if that check fails. Locally, run the
same gate against a host-side server:

```bash
navigator ops assets verify --base-url http://localhost:<web-port>/public
```

### The deployed origin

In the deployment's `deployments/<name>/config.toml`, set `NAVIGATOR_ASSET_BASE_URL` to that deployment's public asset
origin, normally its same-origin `/assets` route (for example, `https://staging.neonlaw.com/assets`). The bucket stays
private; `web` proxies the request, so the browser never needs a raw storage URL. `navigator ops ship` requires this
coordinate before any rollout and verifies it after a full or image-only roll, so a deployment cannot report success
while its public assets are missing.

## Why the images aren't in git

`server/public/img/` is ignored by `.gitignore`. A fresh clone has **empty image slots** — the rest of `server/public`
(Bootstrap, brand SVGs, vendored JS/CSS) stays tracked and still ships in the image, but page images do not. The
deployed app resolves image URLs through `views::assets::asset_url`, which prefixes `NAVIGATOR_ASSET_BASE_URL`; the
staging deployment uses its same-origin `/assets` proxy, while local development defaults to `/public`. The CI KIND
image is the exception: it bakes temporary placeholders under `/public` so browser tests exercise the local path without
requiring GCP credentials or real photo bytes.

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
declares. A manifest photo such as `lake-tahoe` is referenced from Rust rather than from Markdown, so on a fresh clone
with no ADC the blog fills in and that photo stays a broken image. A person or entity avatar is neither: it is uploaded
at runtime through `/app/avatar` and `/app/profile/avatar` (self-service; both POST to the same handler so a relative
form action from `/app/profile` and a nested absolute action both land). The profile page posts that multipart body in
place and refreshes `/app/me/avatar` without leaving the page; a navigation without JavaScript still redirects back to
`/app/profile`. Firm-tier Person avatars, including an Admin upload at `/app/admin/people/{id}/avatar`, are public HTTPS
content: the uploader writes a PNG or JPEG to the public-assets key `people/{id}/avatar.{png,jpg}` and the singular
Surreal `person.profile_image_url` records its public asset URL. That URL comes from `views::assets::bucket_asset_url`,
not `asset_url`. The two differ only in the unconfigured fallback, and only that fallback matters here: a deployment
sets `NAVIGATOR_ASSET_BASE_URL` to `<NAV_BASE_URL>/assets` and `ops ship` refuses to roll without it, so both resolve to
the `/assets/{key}` route there. With no configured origin — the local loop, KIND, `cargo test` — `asset_url` falls back
to `/public`, the crate-bundled static mount, which holds tracked files only and so answers `404` for an object that
exists in the bucket alone. `bucket_asset_url` falls back to the same-origin `/assets/{key}` route instead, which reads
the bucket the uploader just wrote. The profile form says this before submission and limits uploads to 5 MB and 1024 ×
1024 pixels. Client avatars retain their private documents-bucket keys (`people/{id}/avatars/…`), as do Entity avatars
(`entities/{id}/avatars/…`). Dynamic avatars have no manifest entry to pull; application avatar routes retain their
authorization checks, but a public firm-tier Person-avatar URL is intentionally world-readable. Clearing a firm-tier
Person avatar unlinks the row and removes both canonical public variants; clearing a Client avatar leaves its private
object untouched. Until `fetch-referenced` learns the manifest, fetch a manifest photo's variants directly; the widths
and formats are the ones `views::assets` generates:

```bash
mkdir -p server/public/img/lake-tahoe
for w in 400 800 1200; do for ext in avif webp jpg; do
  curl -fsS -o "server/public/img/lake-tahoe/lake-tahoe-${w}w.${ext}" \
    "https://staging.neonlaw.com/assets/img/lake-tahoe/lake-tahoe-${w}w.${ext}"
done; done
```

If you are _curating_ the gallery (adding or replacing a responsive photo), use `build` from the source JPEGs and then
`upload` instead — see [The four commands](#the-four-commands) above. If you are adding a blog hero PNG, put it under
`server/public/img/<slug>/`, verify it locally, then run `assets upload`.
