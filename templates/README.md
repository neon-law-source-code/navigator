# Notations

This tree holds Neon Law Navigator's **notations** — the executable form of the firm's legal work. A notation is one
markdown file that carries three things at once: the **template** (the legal prose the client signs), the
**questionnaire** that gathers the answers that fill it in, and the **workflow** that advances the document from intake
through attorney review to signature, filing, or closing. Templates, questionnaires, and workflows are not three
separate files — they are three faces of one notation.

When a notation is bound to a respondent and a Project it comes to life as a running **Notation** (capital N): the live
matter whose questions get answered and whose workflow advances. That runtime vocabulary is taught in
[`docs/notation.md`](../docs/notation.md); this page is about how the notation tree is organized, named, and checked.

Every notation has YAML frontmatter with `title`, `code`, `jurisdiction`, `respondent_type`, `confidential`, and the
`questionnaire:` / `workflow:` state machines. The body is legal prose with `{{question_code}}` placeholders. Every key
is explained, in plain English and for attorneys, in [`docs/frontmatter.md`](../docs/frontmatter.md).

## Four shelves

The tree has exactly four top-level shelves:

```text
templates/
├── forms/
├── github/
└── neon_law/
```

`forms/` holds government form-backed templates. Its paths mirror the public assets bucket. If the blank PDF is stored
at `gs://<assets-bucket>/forms/united_states/nevada/state/nv__llc_formation.pdf`, the local canonical copy lives at:

```text
templates/forms/united_states/nevada/state/nv__llc_formation.pdf
templates/forms/united_states/nevada/state/nv__llc_formation.fields.toml
templates/forms/united_states/nevada/state/nv__llc_formation.md
```

The markdown file is the catalog card and workflow. Its `code` is the form identity:

```yaml
title: Nevada LLC Formation
code: nv__llc_formation
jurisdiction: NV
origin_url: https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms
respondent_type: person_and_entity
confidential: false
output: form
form: nv__llc_formation
```

`origin_url` is the government page where the blank can be obtained. Git records the exact bytes we vendored; the URL
records where those bytes came from.

`neon_law/` holds the firm's sample onboarding and closing letters:

```text
templates/neon_law/
└── shared/
    ├── onboarding_letter.md
    └── offboarding_letter.md
```

These files are the Firm's confidential work product, and the marks are reserved. **NEON LAW** is a registered trademark
of Shook Law PLLC (U.S. Reg. No. 6,325,650); see the [Trademarks note in the root `README.md`](../README.md#trademarks).
A rebrand goes through the white-label seam.

`navigator validate` rejects any template outside the shelves above. The shelves are the whole surface: a notation lives
under `forms/`, `neon_law/`, or `github/`, and nowhere else.

`github/` holds the engineering intake notations — the questionnaires that gather what a GitHub issue or pull request
needs before it is opened, and the bodies that render the answers into the text that gets posted. They declare `kind:
github` and hold exactly two files, because GitHub opens exactly two things from a questionnaire:

```text
templates/github/
├── create_issue.md
└── create_pull_request.md
```

A GitHub notation borrows the questionnaire grammar the legal notations use, so it reads the same way, but it is not a
legal instrument: it binds to no respondent, declares no jurisdiction or `confidential:` classification, never reaches
lawyer review, and is never imported as a `templates` row. The rules it is held to are the questionnaire-grammar subset
(`N101`, `N103`, `N104`, `N113`, `N115`, `N118`, `N120`) plus `N119`, which pins the shelf's contract.

Both files ask the same two questions before anything else. **The change surface** — `web`, `api`, `infrastructure`, or
`form` — says what the change touches, and each value implies the gate the work has to clear; hover any of them in an
editor to see which. It is asked identically on the issue and the pull request so the answer carries from one to the
other. **The Engineering Council question** records whether the council convenes, because `CLAUDE.md` says councils are
used only when earned and that judgment should be written down rather than remembered. Beyond those two, each file asks
for the narrative it renders: the issue states the observed problem, scope, acceptance criteria, covering tests, and
blast radius; the pull request states what changed, the covering test, the gates that ran, and the walkthrough.

## Naming convention

The `navigator validate` command enforces these with the N-family notation rules:

1. **Only `forms/`, `github/`, and `neon_law/` are valid top-level shelves.**
2. **Every legal template declares `jurisdiction:`**, using a code from `store/seeds/Jurisdiction.yaml` such as `NV`,
   `CA`, or `US`. A `github/` notation declares none — it is engineering intake, not legal work.
3. **Form codes are jurisdiction-first**: `nv__llc_formation`, `us__form_990`. The filename stem, `code`, and `form`
   binding match.
4. **Shared firm codes are role-first**: `onboarding__letter`, `offboarding__letter`.
5. **Every path segment is lowercase `snake_case`**.

Run it before committing:

```bash
cargo run -p cli --quiet -- validate templates
```

This `README.md` is linted like every other workspace README (the validator classifies each file automatically, so there
is no mode flag to pass):

```bash
cargo run -p cli --quiet -- validate templates/README.md
```

## Authoring with live feedback — the LSP

You do not have to run `validate` by hand to find a problem. The same rule engine ships as a small language server,
`navigator-lsp`, that any editor (VS Code, Zed, Neovim, Helix, Emacs) can attach to `*.md`. As you type a notation it
underlines what is wrong, in place:

- a **red** underline is a blocking error — a missing `title`, an unknown `respondent_type`, a `workflow` with no
  `lawyer_review`, a notation that declares only one of `questionnaire:` / `workflow:`;
- a **yellow** underline is a non-blocking advisory — most often a workflow step that is allowed but not built yet.

Hover any underline for the rule and the fix. The server runs entirely on your machine and sends nothing anywhere — the
same confidentiality the `confidential:` key is there to protect. The frontmatter keys it checks are documented for
attorneys in [`docs/frontmatter.md`](../docs/frontmatter.md); editor setup is in
[`docs/lsp/README.md`](../docs/lsp/README.md).

## Adding a form template

1. Put the blank PDF under the bucket-shaped local path:
   `templates/forms/<country>/<jurisdiction>/<office>/<code>.pdf`.
2. Add a sibling `<code>.fields.toml` when the form is fillable.
3. Add a sibling `<code>.md` whose `code` matches the filename stem and whose `origin_url` is the government source.
4. Add the PDF to `forms/src/lib.rs` so the binary embeds the same bytes the repo carries.
5. Run `cargo run -p cli -- validate templates` and the `forms` crate tests.

## Licensing

This tree is licensed on the same terms as the rest of the repository, and deliberately so.

**The notation bodies here are `BUSL-1.1`** — the legal prose, the questionnaire prompts, and the workflow definitions
carried in the same files, exactly like the code that renders them. Adapt them, redistribute them, and make any
non-production use of them; using them where somebody relies on the result — delivering legal services to other
people — is production use and needs a commercial licence from the Firm, as is marketing a product or service to
customers that relies on them. Each version converts to `AGPL-3.0-only` four years after it is published. The prose
and the state machine are the same file here, so a split licence would ask you to work out which half of a line you are
editing; one grant means there is one answer. See [`../LICENSE`](../LICENSE) for the grant and [`../NOTICE`](../NOTICE)
for what the copyright holder says about it.

**The blank government PDFs under `forms/` are not the Firm's to license.** They are works of the issuing state or
federal agency, reproduced here so the binary embeds the same bytes the repo carries. The Firm claims no copyright in
them and grants none. What it does license is its own material beside each one: the catalog card, the `.fields.toml`
field map, and the workflow that fills the form in.

**A template change is still attorney-reviewed.** The licence makes the prose free to reuse; it does not make the prose
safe to change unreviewed. A change here alters a document a real client may sign.
