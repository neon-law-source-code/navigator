# PDF

`pdf` is Navigator's I/O-free document renderer. It compiles Typst into PDF bytes, applies supported redaction styles,
and authors or fills AcroForm fields for official forms.

It serves legal-document workflows that need reproducible output without coupling rendering to storage or a web request.
Keeping persistence outside the crate lets web, workflow, and test code use the same deterministic renderer.

## Rendering a notation template to a PDF

Turn any validation-passing notation template into a PDF on your desk with one command:

```bash
cargo run -p cli -- template render templates/neon_law/shared/letter.md --out /tmp/engagement.pdf
```

The command is `navigator template render`, and it takes the template markdown, not a rendered document. It:

1. **Validates first.** The file runs through the same rule set as `navigator validate`. Any `Error`-severity violation
   stops the render, so a broken template never becomes a PDF someone could send. Yellow advisories print and pass.
2. **Resolves the format** — `--format` on the command line, else the template's `output:` frontmatter field, else
   `plain`. See [Output formats](#output-formats) below.
3. **Fills placeholders.** `--answer code=value`, repeatable, runs through the same notation evaluator as portal preview
   and final document generation. A token with no answer renders verbatim, which is what you want for a blank to fill in
   by hand:

   ```bash
   cargo run -p cli -- template render templates/neon_law/shared/letter.md \
     --out /tmp/engagement.pdf \
     --answer person__client.name="Acme, Inc." \
     --answer custom_text__scope.value="Reviewing and revising the master services agreement."
   ```

4. **Converts Markdown to Typst.** Templates are authored in Markdown; the renderer compiles Typst. `pdf::markdown`
   handles headings, emphasis, lists, block quotes, inline code, and links in between.

This is the offline path — a draft for review, a letter to send by hand. The durable path (retainer workflow →
`generate_pdf__*` → `cloud::StorageService`) uses the same renderer; see
[`docs/notation-authoring.md`](../docs/notation-authoring.md).

## Output formats

`OutputFormat` (in `pdf::format`) is the page chrome wrapped around the body. A template declares its default in the
optional `output:` frontmatter key, validated by rule `N109`; `--format` overrides per render.

- **`plain`** (the default, declared by omitting the key) — page geometry and the firm typeface, one-inch margins, no
  letterhead.
- **`letter`** — the firm letterhead described below, typeset airily. This is the dressing for documents that go out
  under the firm's name: engagement letters, demand letters.
- **`agreement`** — the same letterhead, typeset curtly. The dressing for an executed contract between represented
  parties.
- **`form`** — not typesetting at all. Prints questionnaire answers onto an official government blank (an AcroForm
  fill). See [`docs/gov-forms.md`](../docs/gov-forms.md).

A new form — pleading paper, a fax cover — is a new `OutputFormat` variant plus the Typst preamble that frames it. The
Markdown conversion, the embedded logo, and the font stack are shared, so a variant only describes its own chrome.
`agreement` is the worked example of that seam: it reuses the letterhead block verbatim and changes nothing but page
geometry and spacing.

## The letterhead

`OutputFormat::Letter` and `OutputFormat::Agreement` head the first page with the firm mark, the wordmark in
letterspaced capitals, a rule the full width of the text block, and a contact line beneath it — one shared block, so the
firm's identity is drawn in exactly one place. Every page is numbered `Page N of M`, so a reader can tell when one is
missing from a signed copy.

What differs between the two is density. The letter is typeset deliberately airier than `plain` — wider side margins,
open leading, generous space between paragraphs and above headings. An engagement letter is read once, carefully, by
someone deciding whether to sign it. The agreement tightens all of those and drops headings to body size, because a
contract between represented parties is navigated by section number rather than read through; it also holds every table
unbreakable, so a signature block moves to the next page whole instead of splitting across a page break.

One grey line sits under the rule, carrying every way to reach the firm, voice line first:

```text
+1 510 800 2080  ·  contact@neonlaw.com  ·  www.neonlaw.com
```

**No street address appears on the letterhead.** The firm's postal address is a private-mailbox suite that nothing is
delivered to and no client visits, so printing it on a letter points the reader at a door that does not answer. The
registered address stays in the website footer, where a registered address belongs — and a test asserts the letterhead
never grows one back.

Every line comes from `pdf::Letterhead` — `name`, `phone`, `email`, `web` — and the firm's identity is hard-coded once,
in that struct's `Default`. A letter going out over a lawyer's signature says the same thing every time, so `navigator
template render` takes the default rather than assembling the identity per render. An empty field drops out cleanly: no
dangling middot, and a line with nothing left in it is not emitted at all, so a deployment that publishes no phone still
gets a correct letterhead.

Hard-coding the identity is not permission to let it fall behind the firm's. `cli/tests/letterhead_brand_parity.rs`
holds all four fields against `views::brand::DEFAULT_BRANDING`, so the website and the letterhead cannot publish
different contact details: the voice line moved once while the letterhead kept the retired one, and nothing failed
because the two constants had no relationship to disagree about. That test is the relationship.

## Fonts

Every PDF is set in the firm stack `pdf::BRAND_FONT_STACK` — **GORP Serif** first, **Noto Serif** behind it. GORP Serif
is the face the website is set in, so a rendered letter and the firm's front door read as one brand.

GORP Serif is proprietary TrashType font software licensed separately from this repository, so **its bytes are never
committed**. The renderer resolves it at run time from two places:

- **The host's installed fonts.** On a workstation with the licensed desktop faces installed
  (`~/Library/Fonts/GORPSerif-*.otf` on macOS), letters render in GORP Serif with no configuration at all.
- **`NAVIGATOR_PDF_FONT_DIR`** (`pdf::FONT_DIR_ENV`) — a `PATH`-style list of directories to search. This is the seam
  for a container, which has no installed-font path of its own. Point it at an unpacked `gorp-serif-otf.zip`, the
  archive `navigator assets fonts upload-desktop` publishes and `/app/team/fonts/gorp-serif.zip` serves to the firm.
  Note format: the web faces are WOFF2, which Typst cannot read — this directory needs the desktop OTFs.

Where the licensed faces are absent — CI, a fork, an operator who chose different typography — Typst falls through to
Noto Serif and the document still renders. Noto Serif is the floor rather than a downgrade: two Google Fonts variable
masters are embedded into the binary via `include_bytes!`, so the fallback never depends on the host, and its broad
Unicode coverage (Latin with all European accents, Cyrillic, Greek, Vietnamese) keeps client names rendering correctly
worldwide.

Noto Serif ships under the SIL Open Font License 1.1 (`pdf/assets/fonts/NotoSerif/OFL.txt`). GORP Serif is governed by
the TrashType terms recorded in `server/public/fonts/gorp-serif/LICENSE.txt`; every operator who serves it obtains and
maintains its own license, or replaces it with font software it is licensed to use.
