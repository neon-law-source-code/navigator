---
publish: true
---

# Government Forms: Vendor, Map, Fill, File

Neon Law Navigator fills official government PDF forms from questionnaire answers and files them through a lawyer-gated
workflow. The blank PDF bytes live **only** in the public GCS assets bucket — these are public government documents, so
a public bucket is correct — and the repository keeps the diffable text: the markdown catalog card, the field layer's
text mirror (the `.fields` manifest of the re-authored blank — its `/T` names *are* the answer paths), and a `.sha256`
pin of the canonical blank. The repository path is still the storage contract: the pin at
`templates/forms/united_states/nevada/state/nv__llc_formation.sha256` pins the bucket object at
`forms/united_states/nevada/state/nv__llc_formation.pdf`.

## The Published Offer

The firm advertises this pipeline publicly at `/forms`, and that page is the **only** place on the firm site that posts
a price. Everywhere else an engagement is quoted through `/contact`, because the work is fluid and a posted number would
fit nobody. Two things make the exception hold, and both are load-bearing rather than stylistic:

- **The scope does not move.** The offer is exactly two Nevada formations — `nv__llc_formation` and
  `nv__profit_corp_formation`. Both are re-authored blanks with fixed `.fields` manifests, so every filing asks the same
  questions and fills the same boxes. Nothing about a client's situation is priced into the number.
- **The fee boundary ships with the fee.** The published figure is the *attorney* fee. Nevada's own formation fees are
  several times larger, are set and collected by the Secretary of State, and the firm neither marks them up nor takes a
  share. A page that printed the attorney fee alone would be a misleading fee communication under Rule 7.1 — so the
  separation appears in the tagline, in its own band, and on each card, and is pinned by tests in three places.

Deliberately outside the offer: `nv__business_trust_formation` (vendored and re-authored, but a trust is drafting rather
than a form), `nv__nonprofit_501c3_formation` (an exemption application and governance choices), foreign qualification,
and any formation where the owners have not yet agreed. Those route to a conversation and are quoted as real work.

The engagement is limited-scope under Rule 1.2(c): the firm prepares, reviews, and files the formation documents, and
the representation ends when they are filed. It does not extend to entity selection, equity splits, operating
agreements, or tax treatment. A `$50` filing is still representation — it opens a matter, runs conflicts, and passes
`lawyer_review` before anything reaches a government office.

The copy and its guards live in the `neon` crate's `copy::forms`; the route table entry is `dioxus_app::FIRM_FORMS_PATH`
and the page renders through `firm_marketing_page_router`. The site-wide rule — that this is the *only* page on the firm
host publishing a figure of any kind — is pinned by
`server/tests/neon_routes.rs::forms_is_the_only_firm_page_that_publishes_a_price`.

## Pipeline

```text
government website (`origin_url`)
   │  human downloads / re-authors the blank
   ▼
templates/forms/<country>/<jurisdiction>/<office>/<code>.pdf   (untracked working copy)
   │
   │  navigator template forms sync — uploads, writes the sibling .sha256 pin
   ▼
public assets bucket: forms/<country>/<jurisdiction>/<office>/<code>.pdf
   │
   │  repo keeps: <code>.md  <code>.fields  <code>.sha256   (a <code>.fields.toml is a transient re-author input)
   ▼
forms crate registry (metadata + pin) + manifest resolution
   │
   ▼
portal::retainer_walk::acroform_payload
   │  StorageService::get(object_path) → sha256(bytes) == pin, or fail loudly
   ▼
lawyer_review → workflows::dispatch_generate_pdf → pdf::fill_acroform → pdf::flatten
   │
   ▼
signature / filing
```

The fill path **always pulls**: there are no baked-in bytes and no fallback. A blank missing from the bucket, or one
whose bytes fail the pin, parks the matter with a loud error — `web` never fills against bytes it hasn't pinned. The
verified bytes are staged into the private documents lane at the same key for the worker's `dispatch_generate_pdf` fill,
so what the worker fills is byte-identical to what was verified.

The `.md` file is the catalog card and workflow. It declares the form identity:

```yaml
code: nv__llc_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
form: nv__llc_formation
```

`code`, the filename stem, and the `form:` binding match. `origin_url` points at the government page where the blank can
be obtained. The bucket holds the vendored bytes; the `.sha256` pin records exactly which bytes; the URL records
provenance.

## Vendoring

`navigator template forms sync` closes the loop in both directions, per registry form:

- **With a working copy** at `templates/<object_path>` (untracked — `.gitignore` keeps every `templates/forms/**/*.pdf`
  out of the tree): upload it to the bucket and rewrite the sibling `.sha256` pin to match. Commit the pin (and any map
  change) in the same PR; rebuild so the registry bakes the new pin in. For a re-authored form the working copy's own
  `/T` names must first equal the tracked `.fields` manifest — diverging bytes (say, a pre-re-author blank re-staged at
  the path) are refused before anything uploads or re-pins.
- **Without one**: pull the bucket object and verify it against the pin — and, for a re-authored form, against the
  `.fields` manifest (the blank's own `/T` names re-derived from the pulled bytes and diffed). A missing object or a
  mismatch on either check exits non-zero — the same bytes the fill path would refuse, or names it would silently never
  fill.

`navigator template forms fields <code>` pulls the blank, verifies its pin, and prints the AcroForm `/T` field names one
per line — the ground truth for authoring a `.fields.toml` or re-authoring the field layer (`/T` name = question code,
the sequenced follow-on below). No guessing: these are the names on the exact bytes the workflows fill.

`navigator template forms re-author <code>` (#256 item 1) retires a form's `.fields.toml`: it pulls the blank, verifies
its pin, strips any static XFA packet, and transforms the AcroForm layer so the `/T` names *are* questionnaire state
paths. The map's recorded judgment drives every rename (several hostile names collapsing onto one state merge into one
multi-widget field), every checkbox-pair → radio merge (`custom_single_choice__management_structure`, choice values as
final exports while the source on-states keep the original widget appearances), and every pre-printed literal (`NRS 86`
becomes static content); every field the map never covered lands in the reserved `unmapped__` namespace, so "unmapped"
is a decision the guard checks, not a comment. The transform is byte-deterministic — a CI test re-authors the same input
twice and asserts identical sha-256, so the committed pin is reproducible from the original blank — and refuses loudly
on anything it cannot cleanly express: a `value_map`/`present_in` rule (re-express the judgment in the map first), two
plan entries minting the same final `/T` name (a by-name fill would leave one a silent blank box), two radio members
claiming the same final answer, a checkbox rule with only one of `checked_when` / `on_state`, and a numeric dotted
segment no `row`/`part` list rule minted (the fill path would claim it as a people-row slot). It writes the working copy
plus the sorted `.fields` manifest; visual QA of a sample fill, `navigator template forms sync`, and deleting the
consumed `.fields.toml` remain the human steps.

All three subcommands target `NAVIGATOR_ASSETS_BUCKET` (or `--bucket`) and honor the `NAVIGATOR_STORAGE_ENDPOINT`
emulator override, so the same commands vendor into KIND's Garage `navigator` bucket. Before its first filing, a fresh
environment downloads each blank from its `origin_url` to the working-copy path and runs the sync. An offline mode (a
warmed local cache) is deliberately not built.

## Field Maps and Manifests

Every fillable form is **re-authored** (`nv__llc_formation`, `nv__profit_corp_formation`,
`nv__business_trust_formation`, and `us__naturalization`): it carries a `.fields` manifest — its blank's actual `/T`
names, one per line — and fills with no map at all. `forms::resolve_reauthored` reads each name as its own data path
(`entity__company.name`, `people__managing_members.0.title`, a bare `custom_single_choice__*` radio state), skipping the
`unmapped__` namespace, and hands the result to `pdf::fill_acroform`.

Re-authoring is a one-time transform. A `<code>.fields.toml` map records how each hostile AcroForm `/T` name — real
government field names can be hostile (`undefined`, `City_5`, misspellings baked into the official file), so the map is
data and tests pin it — becomes a canonical answer source; `navigator template forms re-author` (via
`forms::parse_field_map` + `forms::reauthor::plan`) rewrites the field layer so the `/T` names *are* those paths, then
the map is deleted. No form fills through a map at runtime.

Payment-card fields and lawyer-side acceptance pages stay unmapped. If a government PDF reuses the same widget across
sub-forms, the unsafe field stays unmapped until a lawyer review path can handle it deliberately.

## The Field-Name = Question-Code Contract

The fill map is not trusted, it is **checked**. Since the question consolidation
([#233](https://github.com/neon-law-source-code/navigator/issues/233)), a notation's questionnaire states are named
`<type>__<role>` — `entity__company`, `person__registered_agent`, `people__managing_members`,
`custom_single_choice__management_structure` — where `<type>` is one of the canonical seeded question codes in
`store/seeds/Question.yaml`. A field map's answer sources are those same states (directly or by `__role` suffix, exactly
as `fieldmap::answer_for` resolves an answer). So a map that fills a real filing must reference only questions the
questionnaire actually asks, of types the workspace actually seeds.

`forms/tests/question_code_contract.rs` is the guard, run offline in `cargo test` with no PDF or network:

- **`every_notation_state_is_a_canonical_question_type`** — every questionnaire state in each vendored form's `.md` has
  a `<type>` that is a canonical question code (`rules::canonical_question_codes()`, the same source of truth the
  notation-template linter uses).
- **`every_reauthored_field_name_is_a_declared_state_path_or_unmapped`** — the assertion filed onto the names of the
  bytes we ship: every `.fields` manifest entry either carries a declared questionnaire state or sits in the
  `unmapped__` namespace.

A field name that carries no declared state — a re-authoring against the wrong notation, or a notation that drifted —
fails CI here, before a mis-named field can mis-fill a filing.

## XFA Packets

Navigator fills AcroForms. A blank that sets `/AcroForm /NeedAppearances true`, or that also carries a static XFA
packet, is still an AcroForm-backed blank: the XFA XML is stripped during re-authoring and the AcroForm fields remain
the filing surface. A blank that requires dynamic XFA rendering is rejected before filling. The guard recognizes both
the AcroForm `/NeedsRendering true` flag and an XFA config packet that asks for dynamic rendering, because filling those
bytes with a Rust AcroForm writer would otherwise produce a visually blank filing.

The current USCIS N-400 blank is a hybrid static-XFA / AcroForm PDF. The vendored proof uses the USCIS download for the
January 20, 2025 edition and pins the source bytes at
`8b33868ba071e261bf5e8d87d9667860d1f6d2c4de76101eb1e674404a82d909`. Stripping the static XFA packet preserves 440
AcroForm fields and 440 widgets; the re-authored `us__naturalization` blank carries 432 final field names, including 11
mapped questionnaire states and the remaining fields in `unmapped__` for later adjudication. The mapped set is the
first-pass eight (identity, eligibility, residence, marital, contact) plus the applicant's **structured legal name**:
Part 2 Line 1 splits the current legal name into `Family` / `Given` / `Middle`, which one display string can't fill
faithfully, so the three boxes map to `person__client.family/given/middle` — the structured parts the `persons` record
now owns (a matter sets them; an unset part leaves that box for lawyer). The same name is reprinted in the Part 11
certification block, and both occurrences merge onto one multi-widget field per part.

Two intake states stay `unmapped__` on purpose, each because the N-400 wants a *shape* the current intake does not
carry: the total travel-days answer maps to Part 8's six-row trip table (date-left / date-returned per trip), and the
single moral-character prompt maps to Part 9's ~50 itemized yes/no questions. Both are completed by lawyers on the form
until a finer typed slice lands.

## The Guard / Verify Split

CI stays offline; the network truth is checked at vendor time:

- **`cargo test` (offline)** — the question-code contract above, the pin-shape guard (`forms/tests/vendored_forms.rs`),
  and the fill round-trip (`forms/tests/fill_round_trip.rs`), which stages a synthetic blank built from each form's
  `.fields` manifest names (plus the notation's `choices:` for radio groups) in a provider-neutral store and runs the
  full production pipeline against the `cloud::StorageService` seam: pull → verify pin → resolve → fill → flatten. The
  web, CLI, and journey e2e suites stage the same synthetic blanks (`portal::test_support::stage_blank_forms`) under
  their own pins, so the formation flows exercise the pull-and-verify gate end to end.
- **`navigator template forms sync` (network)** — asserts the bucket's actual bytes match the repo pins;
  `navigator template forms fields` reads the field names off those exact bytes. A re-vendor that changes the bytes
  without updating the pin fails here, and the fill path refuses the same bytes in production.

## Sequenced Follow-Ons

The Nevada formation blanks' AcroForm `/T` names **are** the question-code paths. `nv__llc_formation`,
`nv__profit_corp_formation`, and `nv__business_trust_formation` are re-authored by `navigator template forms re-author`,
with their `.fields.toml` maps retired for `.fields` manifests. Profit-corp officer titles are each officer row's own
`title` part, entered at intake; business-trust trustee rows print `Trustee` when an older answer row has no explicit
title. Re-authoring happens on the working copy of each blank; `navigator template forms sync` then vendors it up and
records the new pin.

The filled packet is **flattened** before it is persisted. Because a form's fill state (`generate_pdf__*_pdf`) sits past
`lawyer_review` in every packet's workflow spec, `dispatch_generate_pdf` runs `pdf::flatten` right after
`pdf::fill_acroform`: it paints every value onto the page (text as page content, a checked box as its own appearance
stream), drops the widget annotations (dereferencing the indirect `/Annots` arrays the NV packets use), and empties the
AcroForm `/Fields`. The result freezes exactly what an attorney approved — no downstream viewer can re-edit a value on
the way to a government office, and a viewer that ignores `/NeedAppearances` shows the filled values rather than a blank
form. Overlay text is written in `WinAnsiEncoding` (declared on the overlay font), so accented names render correctly
everywhere; a character outside that encoding fails the flatten loudly instead of filing a garbled glyph.

## Runtime Storage

Blank forms are public assets. `navigator template forms sync` vendors each registry entry to its `object_path` in the
public assets bucket:

```text
forms/united_states/nevada/state/nv__llc_formation.pdf
```

At fill time `web` pulls the blank from that bucket (`cloud::assets_from_env` — `NAVIGATOR_ASSETS_BUCKET`, falling back
to `NAVIGATOR_STORAGE_BUCKET` in the single-bucket KIND/dev topology), verifies the pin, and stages the verified bytes
at the same key in the private documents lane for the worker.

Filled forms are client documents. They are rendered into the private documents bucket at:

```text
notations/<notation-id>/document.pdf
```

Signed copies and certificates use the same per-notation namespace:

```text
notations/<notation-id>/signed-document.pdf
notations/<notation-id>/certificate-of-completion.pdf
```

## Adding A Form

1. Download the blank from the government `origin_url` to the bucket-shaped repo path under `templates/forms/` (it
   stays untracked).
2. Run `navigator template forms sync` — it uploads the blank and writes the sibling `.sha256` pin (tracked).
3. Add a sibling markdown template with matching `code`, `jurisdiction`, `origin_url`, and `form`.
4. Add a sibling field map if the PDF is fillable — `navigator template forms fields <code>` prints the real `/T` names.
5. Add the form's metadata (with its `include_str!` pin) and field map to the `forms` crate registry.
6. Run `cargo test -p forms` and `cargo run -p cli -- validate templates`.
