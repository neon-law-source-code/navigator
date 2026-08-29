---
kind: workshop
title: Using Neon Law Navigator
description: Work one litigation matter end to end, from the retainer template to the client portal.
---

# Using the Navigator Workshop

This workshop uses one local matter from start to finish: **Cruller v. Prine**. The matter code is `sample-litigation`,
its portal application is the [litigation sample project](https://github.com/neon-law-staging/sample-litigation), and
its local data is synthetic. Every attendee sees the same matter through the role assigned to their local account.

The fixture seeds two more matters beside it — `sample-transactional` and `sample-estate` — so the participation-scoped
project list has something in it. The exercises stay on one matter on purpose; the other two are there to be seen from
`/app/projects`, not worked.

## Intro

### Learning objectives

- **Remember** — identify Project, Template, Notation, Workflow, and the unique onboarding / offboarding pair.
- **Understand** — connect each noun to the database row that makes the workflow durable and inspectable.
- **Apply** — open the litigation matter, bind the shared retainer template, and view the client portal application.
- **Analyze** — inspect the notation state and the matter's participation-scoped views.
- **Evaluate** — review the client-facing portal and identify one useful improvement.
- **Create** — make a small, testable change in the sample project and refresh the local portal.

---

Start by naming the four nouns. The workshop keeps every exercise on one matter so the room shares one durable
reference.

### The running matter

The local development fixture seeds three open Projects. This is the one the workshop works:

- **Name** — Cruller v. Prine
- **Code** — `sample-litigation`
- **Matter** — trespass to land, and rescission of the doughnut instrument
- **Repository** —
  [neon-law-staging/sample-litigation](https://github.com/neon-law-staging/sample-litigation)
- **Portal** — `/app/projects/sample-litigation/portal/`

The other two seeded matters are `sample-transactional` (a company on a monthly retainer) and `sample-estate` (an estate
plan). Each carries its own repository and its own portal bundle, mounted the same way.

The Project code is the public URL key. Codes use lowercase letters and numbers separated by single hyphens, so a
project page is always readable as `/app/projects/<code>`.

---

Point out the code in the portal URL. It is the stable, human-readable project identity used throughout the exercise.

## Develop locally

### Start the local room

The Navigator CLI owns the complete local lifecycle. From a New Worktree, run:

```bash
cargo run -p cli -- dev worktree-env up --path "$PWD"
set -a; source .devx/env; set +a
cargo run -p neon
```

The boot command provisions the KIND dependency tier, applies the schema, seeds the sample matters, clones and builds
each sample project, stages every `dist/` output, and writes the generated environment. The host web process reads that
environment on startup, so the real sample applications are ready at their portal links after each boot.

The explicit refresh command uses the same build and staging path when a sample project changes. Naming one matter
refreshes only that bundle, which is the fast loop while iterating on a single app:

```bash
cargo run -p cli -- dev sample-project --project sample-litigation
```

Restart `web` after refreshing so it reads the new staged bundle. The generated `.devx/env` contains
`NAVIGATOR_SAMPLE_PROJECTS_DIR`, the directory every matter's bundle is staged under; source it before starting the host
process.

---

Run the boot commands before proceeding. Confirm that `.devx/env` names the staged sample-projects directory.

### Sign in

The local Rauthy fixture supplies five role-named accounts, all using the password `password`:

| Account | Role | Matter access |
| --- | --- | --- |
| `owner@neonlaw.com` | owner | firm-side matter view |
| `admin@neonlaw.com` | admin | administration surface; participation can be granted there |
| `lawyer@neonlaw.com` | lawyer | firm-side matter view |
| `clerk@neonlaw.com` | clerk | supervised matter view |
| `client@neonlaw.com` | client | client matter view and portal |

Open `$NAV_BASE_URL/auth/login`. Firm accounts land on `/app/team`; the client account lands on `/app/projects`. The
project list and detail page use `sample-litigation` in the URL. The client portal is available at:

```text
$NAV_BASE_URL/app/projects/sample-litigation/portal/
```

The portal is participation-scoped. The client, lawyer, clerk, and owner rows are part of the fixture, and the admin
account is the local administrator used to exercise the participation controls at `/app/admin`.

---

Have each attendee sign in with the role relevant to their work. Keep the browser on the litigation detail page.

## Work the litigation matter

### The four nouns in one workflow

1. **Project** — Cruller v. Prine, the matter that owns the work.
2. **Template** — a versioned Markdown blueprint such as `onboarding__letter`.
3. **Notation** — one client and one Template bound inside the Project.
4. **Workflow** — the states and transitions that move a Notation from intake through review and signature.

The shared retainer template is available in the canonical catalog. A lawyer can bind it through the AIDA catalog:

```text
aida_create_notation(template_code="onboarding__letter", project_id=<sample-litigation project id>)
```

The notation begins in its seeded workflow state. The lawyer reviews the generated work, advances the workflow through
the configured transitions, and the resulting documents remain tied to that Project and its audit trail.

---

Trace one notation from its template through its workflow state. Relate each step back to the same Project.

### One onboarding, one offboarding

Every Project has **one** onboarding notation and **one** offboarding notation. Those two kinds are unique on the
matter: a second retainer is not a second engagement, and a demand letter is not a closing letter.

Onboarding is the letter that opens the matter. Offboarding is the firm-signed closing letter that ends the
representation. Those two shared catalog codes are `onboarding__letter` and `offboarding__letter`. Opening the Project
does not create either notation; a lawyer binds them like any other template. The self-serve doors refuse any other kind
as the matter's first notation.

The CLI seeds both letters. This workshop still binds the retainer through AIDA as one onboarding walk. Close the matter
with the offboarding letter. Do not bind two onboardings on one Project:

```bash
navigator db list templates
navigator site notation create onboarding__letter \
  --project sample-litigation \
  --client-email client@neonlaw.com
navigator site notation create offboarding__letter \
  --project sample-litigation \
  --client-email client@neonlaw.com
```

---

Name the pair on Cruller v. Prine. The onboarding letter is `onboarding__letter`. The closing letter is
`offboarding__letter`. The Projects-list badge reads presence — `onboarding on file` — not execution. A bespoke letter
still counts if it declares `kind: onboarding` or `kind: offboarding`.

### The other notations on a matter

After the engagement is on file, the matter accumulates the work itself. Those later notations are not unique. A
litigation matter may carry many letters and filings. An estate matter may carry a will, a trust, and directives. A
review matter may carry a memo.

The vocabulary is one closed enum, `Kind`, in `rules/src/kind.rs`. A template declares `kind:` in its frontmatter.
Generated PDFs and lawyer uploads reuse the same strings on the asset lane. The notation kinds you add after onboarding:

- `letter` — a letter the firm sends on the client's behalf (demand, notice, settlement)
- `filing` — a document filed with a government body
- `will`, `trust`, `directive` — estate instruments
- `agreement` — a private agreement with a third party
- `memo` — an analytical work product, not an executed instrument

Filed uploads that are not templates use `transcript`, `inbound_contract`, `certificate_of_naturalization`, or
`unclassified`. Content pages (`post`, `workshop`, `event`) and matter dashboards are not notations on the Project.

---

Walk the list against this litigation matter. The retainer is onboarding. A later demand letter is `letter` and may be
one of several. Do not treat `letter` as offboarding.

### Inspect the client portal

Sign in as `client@neonlaw.com`, open `/app/projects`, and select **Cruller v. Prine**. All three seeded matters are in
that list, because the fixture client participates in each one. The detail page keeps the human-readable code in the
address bar:

```text
/app/projects/sample-litigation
```

Select the portal link to open the sample application's bundled client experience. The application is built from the
public sample repository and mounted under the Project's code, which gives the sample a complete path from repository to
matter-specific browser surface.

---

Ask the room to identify what the client can see from this page and what the firm-side matter view adds for the team.

### Make a sample-project change

The sample repository declares its Navigator Project in `navigator.yaml`:

```yaml
project: sample-litigation
```

Edit the sample project, run the refresh command, restart `web`, and reload the portal URL. Boot validates the manifest,
builds the frontend, stages the output, and publishes the generated assets before the entry document. This keeps the
portal tied to the declared Project while the browser reloads the new version.

That manifest is the whole reason three bundles cannot collide. Each is staged in a directory named for its matter, but
boot re-reads the manifest rather than trusting the directory, and refuses a bundle naming a different Project — because
publishing one would put one client's application on another client's portal.

---

Make one small visual change, refresh the bundle, and show the reloaded portal. The manifest name remains
`sample-litigation`.

## Wrap Up

### Verify the room

Run the browser and accessibility gate against the sourced environment:

```bash
cargo run -p cli -- dev browser-e2e
```

The gate signs in the local personas, checks the matter surfaces, and exercises the real local browser path. The Rust
suite and feature walkthrough cover the same fixture and its seeded participation rows:

```bash
cargo nextest run --workspace && cargo test -p features
```

The workshop is ready when the litigation matter appears in the intended role view, `/app/projects/sample-litigation` is
the detail URL, and `/app/projects/sample-litigation/portal/` renders the sample application.

---

Close by verifying the same portal path together. The seeded matter, development flow, and browser proof all meet at one
URL.
