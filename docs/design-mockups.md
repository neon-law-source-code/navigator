# Design mockups

You can design a screen for Neon Law Navigator without writing Rust. Prototype it outside this repository, file it as a
**Linear issue using the Design mockup template**, and a Rust engineer or agent translates it into the real application.
Your deliverable is a specification — a picture of the thing working, plus the source that produced the picture. It is
not a pull request, and nothing you attach is merged, bundled, vendored, or served.

That boundary is the point. Vibing is very good at showing what a screen should feel like and very bad at satisfying
this codebase's authorization, audit, and durability rules. Splitting the work at the issue lets each side do what it is
good at.

## The loop

1. **Prototype outside the repo.** Plain HTML, CSS, and simple JavaScript, in whatever tool you like. One self-contained
   page. Do not clone Navigator, do not add a build step, and do not open a branch.
2. **Record it working.** Capture a GIF of the interaction — clicking through, typing, the error appearing, the save
   landing. A still image is enough only for a genuinely static surface.
3. **File a Linear issue** on the Engineering team using the **Design mockup** template. Fill in every required field —
   see [The template](#the-template) for what it asks and why.
4. **Wait for triage.** The issue is triaged like any other: read, reconciled against the docs and the code, and either
   accepted with an implementation plan or sent back with questions.
5. **Watch the translation pull request.** An engineer builds the screen in Dioxus, links the PR to your issue, and
   posts a walkthrough capture of the real implementation. Compare it against your mockup and say so on the issue if
   something you cared about was lost.

## What to reference while you prototype

The repository's skills, under `.claude/skills/`, are readable on GitHub and are the best short description of how this
product behaves. The useful ones before you design:

- `authorization-model` — who can see what. A screen that shows a client something only lawyers may see cannot be built
  as drawn, and knowing that up front saves a round trip.
- `client-council` — how client-facing decisions get pressure-tested here.
- `marketing-copy` — the rules for public copy, if you are designing a marketing page.
- `web-preview` — how walkthrough GIFs are captured for this repo, so yours matches the house standard.

The translation side is `design-mockup-translation`, which lists the constraints your design will be built against. You
do not need to read it, but it explains why some details change.

## The template

A Linear issue filed with the **Design mockup** template carries these fields:

- **The mockup, rendered.** Required. Drag a GIF of the interaction into the issue description (or a still PNG/JPG if
  the surface is genuinely static) and Linear uploads it as an attachment, embedded inline where you dropped it. An
  issue filed with no image is sent back — the picture is the specification, and nobody can translate prose alone.
- **What the screen does.** Required. The job this screen performs for the person using it, and what the GIF is
  showing step by step. Name the states worth building: empty, loading, error, and success.
- **Target surface.** Required. The page or route this replaces or adds — an `/app/…` route for an authenticated
  screen, or the public path for a marketing page. Write "new" plus your proposed path if it does not exist yet.
- **Brands this applies to.** Required. Navigator serves one property — Neon Law — from one binary, so this names that
  brand.
- **Who sees it.** Required. The authorization tier: Anonymous (a public page, no sign-in), Client (someone the firm
  serves, signed in), Lawyer (an attorney or clerk), or Admin. This decides which lens the translator builds and which
  tests it needs.
- **Data the screen reads.** Required. Every value the mockup displays, in plain words — "the matter's name, its open
  date, and the list of documents with filename and upload date." The translator checks this against the existing read
  API instead of inventing an endpoint, so be specific about what has to be on screen at once.
- **Data the screen writes.** Required. Every change the user can make from this screen, and what should happen after
  each one — what the form submits, what is created or updated, where the user lands. Write "none, read-only" if the
  screen only displays.
- **Reference HTML / CSS / JS.** Required. Paste the source, or link a gist or repository holding it. This is
  **reference material, never code to merge**: no file from it is committed, bundled, vendored, or served. It exists so
  the translator can read your exact spacing, ordering, states, and interaction timing rather than guess at them from
  the image.
- **Anything the translation must preserve.** Optional. The details that would make you say "that is not the design I
  filed" — a specific ordering, a keyboard interaction, wording, a deliberate omission.
- **Required attestations.** Both of the following, checked before you file:
  - Every name, address, email address, and document in this mockup is synthetic or firm-owned. No client information
    appears in the image, the source, or this issue.
  - I understand the attached HTML/CSS/JS is reference material only and no part of it will be merged into this
    repository.

## What makes a mockup easy to accept

- **The GIF shows the interaction, not just the layout.** What happens on click, on error, on empty.
- **The states are drawn.** Empty, loading, error, and success are where translations go wrong. A prototype that shows
  only the happy path leaves four decisions to somebody who did not design it.
- **The data is named in plain words.** "The matter's name, its open date, and each document's filename and upload
  date." The translator checks that against the existing read API instead of inventing an endpoint, so specificity here
  is what makes the screen buildable.
- **The writes say what happens next.** What the form submits, what changes, and where the user ends up.
- **The target surface and brands are stated.** Navigator serves one property — Neon Law — from one
  binary, and a screen rarely belongs on both.

## The two hard rules

**No client data, ever.** Every name, address, email address, matter title, and document body in your mockup must be
invented or firm-owned. Non-firm email addresses use a reserved example domain (`example.com`, `example.org`,
`example.net`); real phone numbers do not appear at all. The template's attestation makes you confirm this, and a mockup
carrying real client information is closed and its attachments deleted. This is the same rule the no-client-data test
enforces on every pull request.

**Your source is reference material.** The HTML/CSS/JS you attach is read, not run. It shows the translator your exact
spacing, ordering, and timing. It is never committed to this repository, never served to a browser, and never becomes a
dependency. The shipped screen is Rust, rendered by Dioxus, and it will look like your design without containing your
files.

## What happens on the other side

The engineer resolves your target surface against the real router, maps each read you named to an existing read endpoint
and each write to the shared command that already performs it, builds the component in the `webapp` crate, adds the
tests, and checks it in a real browser. Details sometimes change: an accessibility fix, an authorization constraint your
prototype could not know about, or a state the real data can reach. When that happens it is explained on your issue
before the pull request merges.
