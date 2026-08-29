# Vibe coding a Project's client portal

Read [`public-contributor-safety.md`](public-contributor-safety.md) first. This is a safe lane for fast experiments:
prototype freely, but share only source and synthetic or firm-owned fixtures. Client data, legal files, real contact
details, and production identifiers never enter Git, Linear, agent transcripts, or another external planning surface.

A Project's **portal** is a React application built with Vite, living in that [Project](glossary.md#project)'s own
private repository, that Navigator serves at `/app/projects/<code>/portal`. It is what the client sees, and it is the
one surface in this product where the fast, exploratory way of working — build the screen, look at it, keep going —
produces the thing that actually ships. Nothing is translated afterward. The React you like is the React that serves.

That makes it the natural home for loose, generative work, and it is why this lane exists as its own document. The rest
of the product is Rust, and its screens are Dioxus components under review; a Project's application tree is JavaScript,
owns its own dependencies, and answers to a much smaller contract. Working quickly there costs nothing, because the
boundaries that matter are enforced outside the code you are writing.

This document is the loop. [`project-repositories`](project-repositories.md) is the repository contract it obeys, and
[`design-mockups`](design-mockups.md) is a different lane for a different problem — see [Which lane](#which-lane) before
starting.

## The loop

Four stages, and each one has a home. Planning is Linear; code, review, and CI are GitHub. Neither surface duplicates
the other.

| Stage | Surface | Skill |
| --- | --- | --- |
| 1. Build it in React | The Project repository | [`vibe-react`](../.claude/skills/vibe-react/SKILL.md) |
| 2. Write the issue | Linear | [`author-linear-issue`](../.claude/skills/author-linear-issue/SKILL.md) |
| 3. Ground it | Linear + docs, source, tests | [`triage-issue`](../.claude/skills/triage-issue/SKILL.md) |
| 4. Land it, then keep it green | GitHub | `create-pr`, then `fix-checks` |

The order is deliberate but not rigid. Vibing *first* and writing the issue *second* is the point: an issue written
after you have seen the screen work names real files, real states, and real traps, which is precisely what §3 of the
working agreements asks for and what prose alone rarely delivers. Build to find out what the work is, then write the
issue that a second agent could execute without asking you anything.

What is never optional is that the issue exists before the pull request opens, and that it was grounded before code
landed. A branch with no issue behind it has nothing to transition on merge and no record of why it was right.

## Planning is Linear, and only Linear

Every issue in this loop is a Linear issue on the **Engineering** team, inside a Linear **project**. There are no GitHub
issues in this lane — not for intake, not for bugs, not for tracking. GitHub holds code, pull requests, review threads,
and checks.

The workspace's own conventions govern, and they are worth reading once before filing anything:

- **No client data in Linear, ever.** Linear is a vendor-operated planning surface read by coding agents. Use a
  synthetic or abstract descriptor; never write a party name, matter code, caption, docket number, address, contact
  detail, legal-file content, or production identifier. The repository test does not cover Linear, so the boundary is
  the control.
- **Initiative → Project → Issue → Sub-issue.** A project finishes; if it cannot finish it is an initiative. An issue is
  one concern and one pull request — if describing it needs the word "and", it is two issues.
- **An issue is ready when a coding agent can execute it without a second conversation.** That means all five sections:
  observed problem, grounded scope with real files named, acceptance criteria someone else can check, the covering test,
  and the known traps. An issue that says only what somebody wants stays in Triage.
- **The description holds current truth; comments hold the trail.** Rewrite the description freely so it is always
  current, and leave a dated comment for every material change. A reversal's reasoning usually outlives its conclusion,
  and a bare rewrite destroys exactly that.
- **Branch names carry the issue.** `nick/eng-123-slug`, so Linear links the branch, the magic word closes the issue,
  and the merge transitions status. Never hand-transition an issue a merge should have moved.

Status runs `Triage` → `Backlog` → `Todo` → `In Progress` → `Done`. Nothing is handed to an agent from Triage.

## Stage 1 — build it in React

The full authoring rules are [`vibe-react`](../.claude/skills/vibe-react/SKILL.md). The shape of the constraint, though,
belongs here, because it is what makes vibing safe:

**You own the screen. You do not own the data.** A Project repository holds template and application source plus
checked-in configuration, and never a matter-data backend of its own. Reads go through Navigator's `/api` read clusters
and writes go through the one REST command boundary in [`command-boundary`](command-boundary.md). Authorization is
decided by Navigator from Project participation before your bundle is ever served, so there is no client-side
authorization decision to get wrong.

Everything you would normally have to be careful about — who may see this, whether this write is allowed, where the
legal file lives — is somebody else's enforced invariant by the time your component renders. What is left is layout,
states, copy, and interaction, and those are the things worth iterating on quickly.

Two hard rules survive into the repository, and both are mechanical:

- **No legal files, no client data.** Git never stores legal files; Navigator-managed systems and approved file stores
  do. Fixtures are synthetic or firm-owned, non-firm email addresses use a reserved example domain, and no phone numbers
  or production identifiers ship.
- **The mount is derived, not declared.** The repository's name is the Project code and the segment is the literal
  `portal`, so the base is `/app/projects/<code>/portal/`. Nothing names it twice. Absolute paths in source are refused,
  with one deliberate exception — the link back to `/app/projects` — so build the rest from `import.meta.env.BASE_URL`.

## Stage 2 — write the issue, after you have seen it work

Now that the screen exists, write the Linear issue the way
[`author-linear-issue`](../.claude/skills/author-linear-issue/SKILL.md) requires: grounded in source, citing
`file:line`, proposing nothing that already exists.

Vibing gives you an unfair advantage here. You are not predicting a blast radius, you are reading one off a working
diff. Name the components you actually touched, the read endpoints you actually called, the states you actually built,
and the trap that actually cost you an hour — a base-URL mistake, a missing empty state, a read that returned a shape
you did not expect. That last section is the one people skip and the one that saves the next agent the most time.

Write the issue to describe **the work**, not the prototype. If the exploration produced three screens' worth of change,
that is three issues, because it will be three pull requests.

## Stage 3 — ground it before anything lands

[`triage-issue`](../.claude/skills/triage-issue/SKILL.md) is the procedure, and the discipline it enforces is worth
stating plainly: **the repository is almost always further along than the backlog says.** Work written from issue text
alone is regularly work that is already done.

Grounding a Project-application issue means reading, in this order:

1. [`glossary`](glossary.md), then the narrowest relevant doc from [`index`](index.md).
2. [`project-repositories`](project-repositories.md) — the repository contract, and what the mount gate does and does
   not prove.
3. [`command-boundary`](command-boundary.md) and the `/api` read clusters — whether the read or write this screen needs
   already exists. If it does not, that is a Navigator dependency and a separate issue, not a JSON endpoint you add.
4. [`access-model`](access-model.md) — which participation and role actually reach this mount.
5. The Project's own coordinates, which are derived rather than registered:

```bash
navigator site projects doctor --project <project-code>
```

There is nothing to register. A Project has one repository, named for its code, and one portal, mounted at that name
plus the literal `portal`. `navigator site projects doctor` reports both coordinates; whether the repository exists yet
is a separate question, and a coordinate that names nothing is a legitimate state rather than an error.

Triage ends at a plan comment on the Linear issue. Implementation is a separate action that starts in its own worktree.

## Stage 4 — land it, then keep it green

Open the pull request against the Project repository's `main` with [`create-pr`](../.claude/skills/create-pr/SKILL.md),
carrying the Linear magic word in the body so the merge transitions the issue.

Then the review loop takes over, and it has two distinct inputs that people tend to collapse into one:

- **A failed check** is a machine finding. Read the log, find the *first* actionable failure rather than a downstream
  cancellation, reproduce it locally, and make the smallest root-cause fix.
- **An inline review comment** is a human or reviewing-agent finding on a specific line. Read the thread and the code at
  the pull request head, decide from evidence whether it is valid, and fix only what that comment asked for.

Both are [`fix-checks`](../.claude/skills/fix-checks/SKILL.md), which is action 5 of
[`agent-workflows`](agent-workflows.md) plus the comment half of action 4. The rule that keeps this loop from drifting:
one finding, one fix, one reply carrying the proof. Bundling unrelated cleanup into a review round is how a small pull
request becomes unreviewable.

A pull request touching authorization, billing, notation bodies, or the store is reviewed by a human before merge. That
is not negotiable, and a Project's portal reading matter data is close enough to that line to assume it applies.

## Which lane

Two lanes take a prototype and produce a shipped screen, and they are not interchangeable. The question is not how you
built the prototype — it is **which surface the finished screen lives on**.

| | A Project's portal | Navigator's own screens |
| --- | --- | --- |
| Ships as | The React you wrote | Rust, rendered by Dioxus |
| Lives in | `<org>/<code>`, under `portal/` | The `webapp` crate |
| Served at | `/app/projects/<code>/portal/` | A Navigator route |
| Prototype is | The implementation | Reference material, never merged |
| Intake | A Linear issue | A `design-mockup` issue |
| Read | This document | [`design-mockups`](design-mockups.md) |

If the screen belongs to one Project and reads that Project's data through Navigator's APIs, it belongs in that
Project's portal and you are in the right document. If it is part of Navigator itself — a portal page, a lawyer surface,
a marketing page, anything at a Navigator route — the React is a prototype and it will be translated to Dioxus by
[`design-mockup-translation`](../.claude/skills/design-mockup-translation/SKILL.md).

## What is not built yet

This loop is honest about its own seams. Today the repository shape, the scaffold, the CI gate, and the route ship: one
Each Project repository holds `templates/` and `portal/` side by side. `navigator site projects repository` scaffolds
and validates it; `.github/actions/validate` is the one gate, `navigator site projects doctor` verifies a machine, and
Navigator routes `/app/projects/{code}/portal` through Project participation authorization.

The CI publish wiring now exists: `.github/actions/application-publish` uploads a built bundle to `<code>/portal/` in
the deployment's private applications bucket — see
[`project-repositories`](project-repositories.md#publishing-the-built-bundle). Two pieces are still open, and an issue
in this lane should reference them rather than assume them:

- **Provisioning the publish target.** The applications bucket and the Project's own `nav-pub-<code>` publisher identity
  are not yet provisioned for any deployment, and the shared reusable-workflow home in `ux/core` is not yet wired, so
  the caller transcribes the job for now.
- **Serving that bundle.** The route resolves and authorizes, and then answers 404, because there is nothing published
  to stream. A participant and a nonparticipant get the same non-disclosing response, so the status code discloses
  nothing about which one they are.

Until those land, a portal builds, tests, and passes the mount gate in CI, and its bundle is not yet served by a
deployment.
