---
name: portal-chrome
description: >
  How a Project portal is allowed to look: it wears `@neon-law-source-code/navigator-ux` as shipped — the Neon Law
  teal, `NavigatorShell` with `NavigatorNavbar` and `NavigatorFooter`, and a way back to the matter show page — and it
  never repaints the palette or hand-rolls the frame. Trigger whenever a task touches a portal's styling, colours,
  header, footer, navigation, or page frame, and before adding any stylesheet to `portal/src/styles/`. Synced verbatim
  into every Project repository's `.claude/skills/` by `navigator site projects repository sync-skills`; this canonical
  copy lives in Navigator's own `.agents/skills/`.
---

# The portal wears the library, unmodified

A Project portal is served by Navigator, from Navigator's own host, at `/app/projects/<code>/portal/`. A reader
crosses into it from the matter show page one segment up and does not experience that as leaving the product. So the
portal has to look like the product, and the way it does that is by rendering `@neon-law-source-code/navigator-ux`
exactly as the library ships it.

That is the whole rule. Everything below is a consequence of it.

## The palette is teal, and there is no brand layer

The library ships one identity: the Neon Law teal, as `--nav-teal-*` behind the `--nav-color-*` semantic aliases.
`www.neonlaw.com` is that library with nothing painted over it. A portal is the same library on the same host, so a
portal is teal too.

**Do not add a brand layer.** Specifically, do not create `portal/src/styles/brand.css`, do not redeclare
`--nav-color-primary` or any of its siblings, and do not import a stylesheet between the library's and the app's own.
A `:root:root` block that repaints the ramp is the exact shape of the mistake — it wins on specificity, it applies to
every component at once, and the result is a page that looks deliberate and belongs to nobody.

This is written down because it happened. Portals across the fleet carried a `brand.css` that repainted the teal
orange, and its own header claimed it was byte-identical everywhere and had to change in all portals or none. Neither
half was true: there were two variants of it and at least one portal carried none. A convention that is only in a
comment is a convention that drifts, which is why this one is a synced skill and a validate finding instead.

If a colour genuinely needs to change, it changes in `neon-law-source-code/navigator-ux`, where it is reviewed as a
visual regression across every consumer. It does not change in one portal.

## `app.css` is the seam, and it stays nearly empty

`portal/src/styles/app.css` is imported last and wins on equal specificity. It is for the rare thing that has to
disagree with the library — a grid the library has no component for, say. It is **not** for tokens. A token restated
there is a token that stops tracking the palette, and it will be the one element on the page that looks wrong after
the next library release.

Anything added there earns a comment saying what it disagrees with and why the library could not do it.

## The frame is `NavigatorShell`, not a hand-rolled header

Use the chrome the library ships:

- **`NavigatorShell`** wraps the page, with `header` and `footer` slots.
- **`NavigatorNavbar`** is the header. Give it `brand`, `destinations` for the portal's own routes, and — this is the
  part most often missed — a `brandHref` that leaves the portal.
- **`NavigatorFooter`** is the footer: a `legal` line, `links`, and `release`.

Do not build a bar out of primitives, and do not leave the portal with no way out. `PageHeader`, `CaseHead`, and
`Hero` are page headings and none of them is chrome; a page heading inside the shell is right, a page heading standing
in for the shell is not.

## The way back is the matter show page

`brandHref` points at `/app/projects/<code>` — the matter show page, one segment above the mount. Navigator serves it,
the portal does not, and it is the authority on everything a portal declines to state: the status, the participations,
the answers, and the documents.

Derive it from the mount rather than writing it out:

```ts
export function matterShowPath(base: string): string {
  return `/app/projects/${projectCode(base)}`
}
```

`import.meta.env.BASE_URL` is the mount and the only source of an in-app path. A Vite base rewrites module and asset
URLs and never an href written by hand, so a literal path survives the build pointing at whatever Navigator serves at
that address — which, for a portal, is another Project's matter.

Every other href stays under the mount, with **no trailing slash**: Navigator's `asset_path_is_safe` requires every
`/`-separated segment to be non-empty, so `notations/` is refused *before* the single-page-application fallback, and
the refusal is the same non-disclosing 404 a nonparticipant receives. A broken link looks exactly like a permission
boundary.

## Two component traps worth knowing before you hit them

- **`Runs` is inline.** It emits spans and no block of its own, so two adjacent `Runs` render as one run-on line with
  no separator. For two lines of text, use `Prose` with one entry per paragraph.
- **`FactCard` and `DataTable`'s `empty` slot wrap their children in a paragraph.** A block component nested inside is
  invalid markup a browser silently reflows. Put a `Runs` in a `FactCard`, and use `Empty` for a list with nothing in
  it rather than the table's own sentence-sized slot.

## Themes are the library's too

`ThemeProvider` reports the operating system's colour scheme; the tokens do the rest in a media query. There is no
toggle, no stored choice, and no pre-paint script to add. An SVG drawn into a portal uses `var(--nav-color-…)` for
every fill and stroke — a hex value there is the one mark on the page that will not follow the theme.

## Before you open a pull request

```bash
navigator site projects repository validate . --repository <code>
cd portal && pnpm check
```

The `--repository` flag is not optional in practice: without it the CLI derives the code from the checkout directory,
which fails a repository that is correct.
