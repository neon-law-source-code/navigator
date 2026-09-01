# Project workspace and repository contract

Each Navigator [Project](glossary.md#project) coordinates five distinct surfaces. They are not interchangeable stores:

| Surface | Authority | Contains |
| --- | --- | --- |
| Documents bucket | Working files | Path-like keys under `projects/<code>` in the private documents bucket |
| Google Drive | Ingest dropbox | Files people drop in; Navigator copies them into the documents bucket |
| Navigator | Matter record | Project identity, participation, Notations, and asset provenance |
| Project repository | Source control | Notation templates and client-portal source only |
| Served client portal | Authorized application | The Project's client-facing surface |

Git never stores legal files or client data. Navigator-managed systems and approved file stores do. A Project's deletion
handoff contains legal files only; it does not include the repository, portal source, CI output, or operational history.
Read [`public-contributor-safety.md`](public-contributor-safety.md) before preparing fixtures, planning work, or sharing
an experiment: source control and external planning surfaces carry only firm-owned or synthetic source material.

## One repository per Project, recorded as a URL

A Project has **one** repository, and the Project records **where it is** as a whole URL in `project.repository_url`. It
holds that Project's notation templates and its client portal side by side:

```text
<the Project's repository>
├── .github/workflows/gate.yml
├── portal/            # React + Vite; the client's portal
├── templates/         # *.md notation blueprints
├── AGENTS.md
├── CLAUDE.md
├── LICENSE.md
└── README.md
```

**The URL is stored, never composed.** Navigator does not build `https://<host>/<org>/<code>` from a deployment-wide
forge host, because a Project's source is not the Firm's to place: it may sit on GitHub, on GitLab, on a self-hosted
remote, in an organization the Firm does not own — one Project per forge if that is how the work arrived. A composed
coordinate can express none of that, and worse, it always *exists*, so it produces a confident link for a Project that
has no repository at all.

The value is validated rather than trusted, by `store::projects::is_valid_repository_url`: `http(s)` only, a non-empty
host and path, no whitespace, and no embedded credential. That URL is handed to `git clone` and rendered to a lawyer as
a link, so a `file://` value would read the serving host's disk and a `user:token@` value would put a secret in a column
that is rendered into a page and logged.

The Project code is the stable Navigator `projects.code`, and the documents-bucket prefix is `projects/<code>`. It is
also the repository's own directory name, by the convention the next section states — that equality is why the slug
rules are what they are: lowercase letters, digits, and single hyphens, alphanumeric at both ends, at most 80
characters. A checkout and macOS are case-insensitive, so uppercase would let one directory answer to two codes; one
separator keeps the mapping an equality check rather than a normalization. The code no longer names a Drive folder —
per-matter Drive folders are being retired (see [the glossary](glossary.md#project)) — and `project.repository_url`
itself remains a stored URL Navigator never composes from the code (above).

`new` is refused as a Project code. `/app/projects/new` is Navigator's matter-open form, so a Project coded `new` would
collide with a literal route. Which side of a genuine collision wins depends on route registration order, so the code is
refused rather than the precedence reasoned about — in `store::projects::is_valid_code` and in an `ASSERT` on
`project.code`, because a Rust check only guards the write paths that call it.

`navigator` is refused too, for a different reason: not a route collision but a repository one. Because the repository
name *is* the code, a matter coded `navigator` in the Firm's own organization would name Navigator's own source rather
than a matter's, and every rule that treats a Project repository as client-adjacent would then be pointed at the
product. `cloud::workspace::NAVIGATOR_REPOSITORY_URL` names that one repository — one host, one organization, the same
on every deployment forever, which is exactly what a Project's repository never is — and
`cloud::workspace::RESERVED_PROJECT_CODES` carries both refusals. The shared gate action refuses the same two names, and
`cli/tests/project_gate.rs` holds the two lists identical in both directions.

## The repository name is the Project code

A Project code is **lowercase letters, digits, and single hyphens**, alphanumeric at both ends, at most 80 characters —
no uppercase, no underscores, no other punctuation, no spaces. `store::projects::is_valid_code` is the one definition
and [the glossary](glossary.md#project) carries the rationale for each restriction. The code is the matter's whole
public identity: its show page, its client portal, and its repository name are all that one word — and it is
**immutable**, chosen once at matter-open and never changed. A lawyer supplies the stem; Navigator appends a short
generated suffix (`store::projects::code_from_name`) so two matters can never collide on a hand-picked stem, the way a
hand-picked permanent identifier once could.

```text
/app/projects/<project-code>            the matter's show page — never its internal UUID
/app/projects/<project-code>/portal/    the client portal this repository publishes
```

**A repository declares its Project in a root manifest, `navigator.yaml`, keyed `project:`.** That manifest is part of
the layout (`ALLOWED_ROOTS` in `cli/src/projects/repository.rs`), and it is the one spelling — the earlier `.yml`
extension, keyed `name:`, is retired, and a checkout still carrying it reads as an unparsable manifest. The same file
and the same reader serve both a Project repository and a staged sample-project bundle:
`store::sample_project::MANIFEST_FILE` and `cli/src/projects/repository.rs`'s `PROJECT_MANIFEST` name the identical
string on purpose, so a rename of one cannot leave the other stale. Unknown keys, such as `host:`, are ignored rather
than refused, so a downstream deployment table can add its own without breaking this gate.

**The manifest is what `.github/actions/application-publish` reads.** `cli/src/projects/repository.rs`'s own
[`validate`] still takes the code from the checkout directory — it runs inside one repository's own CI with no access to
the live row, so it cannot referee a disagreement between the two, and `navigator site projects drift` is where that
disagreement is reported instead. The publish action is different: it uploads to a bucket prefix, and the prefix a
Project repository declares for itself is the one that should win. Vite still derives its own build-time base from the
checkout directory (`basename(resolve(__dirname, '..'))`), so a repository whose manifest names a code other than its
own directory name only works if its portal was built with a matching override — the downstream mount check on
`index.html`'s Vite base is what would catch a mismatch either way, so a wrong or malformed code cannot silently
publish. `application-publish`'s `repository:` input is kept as an override for a checkout that carries no manifest,
never the primary source.

Every repository shipping today keeps the manifest and the directory name aligned by convention: each sample repository
is named for the code it mounts on, so `neon-law-staging/sample-litigation` publishes as `sample-litigation`. That is a
naming discipline, not an enforced rule — nothing stops a repository from being named for something else, and the
manifest is what settles which code wins.

The trailing slash is load-bearing twice: Vite joins asset URLs directly onto the base, and Navigator redirects the bare
mount to the slashed form.

**Every path below the mount resolves, not only the files the build published.** A portal writes its own section links
as `<base><slug>/`, and a reader bookmarks and refreshes them, so Navigator is asked for paths no publish ever wrote.
One request resolves in order: the path itself when it names a published object, then that path's `<dir>/index.html`,
then the bundle's root `index.html`. A build of many pages therefore serves its own pages, and a build of one bundle
serves its entrypoint so the client-side router renders the route. Reaching for the directory index first is what keeps
the entrypoint a fallback — without it a multi-page build answers every page with the wrong document. Every `index.html`
is served `no-store`, because it names the build's content-hashed assets and is never hashed itself; those assets cache
for a year.

**The extra `portal` segment is the point.** Navigator's matter show page is `/app/projects/<code>` and the client
application is `/app/projects/<code>/portal/`. The Project code is the stable lowercase-kebab URL slug; the internal
UUID is not exposed in the show-page route.

During local development, `navigator dev up` and `navigator dev worktree-env up` clone, build, and stage each sample
project before writing `.devx/env`. The host `web` process therefore starts against the same refreshed portal bundle for
every developer.

## The organization is configuration, not a name in source

Navigator spells no organization in its source, and one forge host: the named default
`cloud::workspace::DEFAULT_GIT_HOST`. `NAVIGATOR_GITHUB_ORG` names the organization this deployment's *own* automation
occupies, and `NAVIGATOR_GIT_HOST` names the host it lives on. The two are one `(host, organization)` coordinate — where
this deployment's Project repositories are created, and the boundary `ops github setup` refuses a governance write
outside. Two organizations are admissible on a governance write: the public one holding Navigator itself, and this
deployment's own. A remote in neither is refused before a token is read.

Only the host carries a default, and only because it has a right answer. An organization has none, so a named deployment
must state it. `cli/tests/forge_coordinate_retired.rs` is the guard that keeps the organization out of source, admits
exactly one spelling of the host — that constant's own declaration — and keeps neither from being composed into a
Project's URL.

**Neither names a client matter's source.** That is `project.repository_url`, which is data. A deployment's organization
and a Project's repository are independent: a deployment can serve a matter whose source lives in a client's own GitLab
group.

| Deployment | GCP project | Organization | Drive root |
| --- | --- | --- | --- |
| Production | `neon-law` | `neon-law` | `Projects` |
| Staging | `neon-law-stg` | `neon-law` | `Staging Projects` |

The active deployment is identified by `NAVIGATOR_GCP_PROJECT_ID`. It is deliberately not `NAVIGATOR_ENVIRONMENT`, which
is a two-valued dev/production switch and cannot name a deployment.

**One string means two different things across those two vocabularies: the organization `neon-law` is staging, while the
GCP project `neon-law` is production.** That inversion is accepted rather than accidental — the organizations are named
for the entities and the GCP projects for the deployments. It is the single most likely way to ship to the wrong place,
so it lives in the configuration an operator reads rather than in source where it would have to be remembered.

### An absent repository is legitimate

A Project may record no repository at all, and that is correct rather than degraded: a matter opens before anyone
creates its source, so `project.repository_url` is nullable and the lawyer matter page simply omits the pointer.

Nothing invents one to fill the gap. That is the whole difference from the derivation this replaced — a composed
coordinate always existed, so it rendered a confident link whether or not the repository did, and when the forge host
fell back to a public default it aimed that link at a namespace the Firm does not control. `ops github setup` documented
having no public fallback while the pointer that actually served users had one.

| State | Answer |
| --- | --- |
| `repository_url` recorded | The lawyer sees it verbatim. Never verified — the target may not exist yet. |
| `repository_url` absent | The pointer is absent. Not an error, and nothing is composed. |
| A value that is not an `http(s)` URL with a host and path | Refused at the write, not stored. |

### Reconciling the rows against what they record

`GET /app/api/project-repositories` reports where a Project row and its `repository_url` disagree, across every matter
in the deployment. It reads rows rather than checkouts, and both halves of that are deliberate.

A scan of local clones would make every answer conditional on the operator having cloned the whole fleet, and silently
wrong when they have not — in a Project repository's own CI run exactly one checkout is present, so every other matter
would read as a repository that is gone. And the matter list is a lens rather than an inventory:
`store::access::visible_projects` scopes to the caller's participation rows for **every** firm tier, Owner and Admin
included, so a read through it cannot tell "no such row" from "not yours".

So the door is admin-tier and reads all rows directly. That tier is the control rather than a precaution: reading every
matter is the privileged reach the matter surface refuses to grant silently, so it is a door an administrator navigates
to. It discloses one code and one repository URL per matter, and no matter content.

| Finding | Severity | Needs the forge pair |
| --- | --- | --- |
| `repository-name-is-not-code` — the recorded URL names a different repository | fail | no |
| `records-navigator-itself` — the row records Navigator's own repository | fail | no |
| `duplicate-repository-url` — two matters record one repository | fail | no |
| `repository-url-invalid` — a value today's write gate would refuse | fail | no |
| `no-repository-url` — the row records none | warn | no |
| `repository-outside-deployment-forge` — recorded somewhere this deployment would not have created it | warn | yes |

Every **failing** finding is computable from one row and a rule, because the code is the repository name. Only the last
needs configuration, and it is a warning by design: a Project's source may live on any forge, in an organization the
Firm does not own, which is the state a stored URL exists to permit. Where a deployment has no pair configured, the
report carries `compared_against_deployment_forge: false`, so an absent warning is never read as agreement.

Findings serialize internally tagged — each carries its own fields beside its `kind` and `severity` — so a consumer
reads what it needs by name rather than parsing a sentence.

### Provisioning the three handles

Opening a Project records its identity, then `store::project_surfaces` creates or adopts the three external surfaces
that identity names: the documents-bucket prefix `projects/<code>` (a key convention; nothing is written), the Drive
ingest folder named for the code, and one private source repository named for the code. Each step is idempotent. A
folder or repository that already exists is adopted. A recorded `repository_url` is left alone, so a Project whose
source lives on another forge is not moved. Missing Drive or forge configuration skips that surface rather than failing
the matter open.

`POST /app/api/project-surfaces/{id}` is the admin retry for a failed or legacy row. It carries its own noun rather than
sitting under `/app/api/projects/`, because that prefix's GET rule admits any authenticated caller up to five segments.
CLI: `navigator site projects surfaces reconcile --project <code>`; Project participation is never copied onto the
forge.

## The CI gate

One composite action verifies the layout, the portal build, and the mount, consumed identically by every Project
repository in every organization:

```yaml
- uses: actions/checkout@<sha>  # v7
- run: pnpm --dir portal build
- uses: neon-law-source-code/navigator/.github/actions/validate@YY.M.D
  with:
    version: "YY.M.D"
    project_repository: true
```

It carries no organization, host, deployment, or client name, because none of those vary: the mount is the repository
name, which is the Project code, plus a literal segment. A forge host never appears in a Vite base, which is why a
repository may move between forges without touching the gate. `cli/tests/project_gate.rs` pins the shell against the
Rust definitions it transcribes, because bash cannot call Rust.

`navigator site projects repository scaffold` generates the shape every Project repository converged on by hand before
this generator caught up: three feeder jobs — `lint`, `verify` (typecheck, test, build), and `notation` (the snippet
above) — fanned into one required check. Each feeder job runs unconditionally and no-ops over a half this repository
does not carry: every portal-specific step carries a run-time `hashFiles('portal/package.json') != ''` condition,
because `scaffold` writes the gate before the portal exists and the same file must keep working once the portal arrives
later from the vibe-coding lane.

**There is no path filter, and that is deliberate.** A filtered job that skips reports success for work it never did,
and a required check a skip can satisfy is not a gate. So every job always runs and each half no-ops over a repository
that does not carry it, rather than being skipped. The required job is spelled `ci`, which is the one context `navigator
ops github setup` binds — and, because a **skipped** job reports no result at all, that job asserts each feeder job's
`result` explicitly rather than trusting a bare `needs:`, which a skip would satisfy silently.

What the gate proves:

- The layout is source-only. Client uploads, answers, generated documents, secrets, dependencies, and build output are
  refused by path and by extension.
- Every direct `templates/<code>.md` passes the notation rules, and each template's `code` equals its filename stem.
- Where a `portal/` exists, it is a Vite workspace — a `package.json`, an `index.html`, and a lockfile. The lockfile
  flavor is not constrained and there is deliberately **no dependency allowlist**: third-party libraries are the point,
  and Node never enters the Navigator workspace.
- The built `index.html` is mounted at `/app/projects/<code>/portal/`, so a base that never reached the build fails here
  rather than in production.
- No absolute path in `portal/src/` escapes the mount. A Vite base rewrites module and asset URLs and never an `href`
  written by hand, so a literal in-app path survives the build pointing at whatever Navigator serves there instead.
  Navigator's own namespaces, `/app/` and `/auth/`, are the deliberate exception: a portal links back to `/app/projects`
  and out through `/auth/logout`, and those are outside the mount on purpose.

Pin the action to an exact immutable release tag (`YY.M.D`, or `YY.M.D-hotfix.N`), never `main` or `latest`. Publishing
a rolling pointer is allowed; consuming one is not. The tag must also be one this repository actually published: a
`uses:` at a ref that does not exist fails the run outright with "unable to resolve action", and unlike a renamed
repository — which GitHub redirects, so the old spelling keeps working — a missing ref has nothing to redirect to. The
shape rule is machine-checkable and `validate` enforces it; whether the tag exists is not, which is why `scaffold`
derives the pin from a release rather than accepting a version someone typed from memory.

## Publishing the built bundle

The gate proves the bundle; a second composite action publishes it.
`neon-law-source-code/navigator/.github/actions/application-publish@YY.M.D` runs after the gate, in the same job, and
uploads `portal/dist/` to `<code>/portal/` in the deployment's private `<deployment>-applications` bucket, which
Navigator streams object-by-object. Objects land **flat** under that prefix; the action reads `<code>` from the
repository's own `navigator.yaml` manifest, falling back to `github.event.repository.name` (its `repository:` input)
only when no manifest is present. The mount check the same step runs against the built `index.html` is what keeps a
wrong or malformed declared code from silently publishing: the object prefix must match the Vite base the portal was
actually built with, wherever the repository is hosted.

It carries no organization, host, or client. The three coordinates it cannot derive are passed as repository
**secrets**:

| Secret | Value |
| --- | --- |
| `NAVIGATOR_APPLICATIONS_BUCKET` | the deployment's private applications bucket, e.g. `neon-law-applications` |
| `NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER` | the full Workload Identity provider resource, pool and provider id included |
| `NAVIGATOR_APP_PUBLISHER_SERVICE_ACCOUNT` | `nav-pub-<code>@<project>.iam.gserviceaccount.com`, this Project's own |

**Secrets for disclosure reduction, not for access control.** All three are public identifiers and knowing them grants
nothing; the Workload Identity binding on Google's side is the gate, and it is the only one. They are secrets because
two of them carry the deployment's GCP project identity in their own text — the service-account email *is* the project
id, and the provider resource *is* the project number — and the bucket is named `<deployment>-applications`. Project
repositories are public and so are their Actions logs, and GitHub redacts secrets from logs while leaving variables in
full, so passing these as variables published a map of the organization's GCP topology beside every build. Nothing about
this substitutes for the binding: a change that needs to widen or narrow real access belongs in
`cli/src/devx/gcp/app_publisher.rs`, and the binding must not be weakened on the belief that this covers it.

Because GitHub redacts a secret's exact text and not the identifiers inside it, the action additionally registers
`::add-mask::` for the bare project id and project number, decomposed from those two coordinates, before any step runs —
`gcloud` prints them on their own, where neither is the whole of a registered secret.

Authentication is keyless: the job mints a short-lived OIDC token from GitHub's issuer
`https://token.actions.githubusercontent.com` and federates it into the publisher, so no service-account key exists.
That issuer is a property of the provider resource, not a workflow parameter. Because the whole resource travels in the
secret, the pool and provider id are the deployment's business and never a name a Project repository knows.

**On `neon-law-stg` that resource is the `github` pool's `github-oidc` provider, which is not what
`cli/src/devx/gcp/app_publisher.rs` provisions** — it creates an `app-publisher` pool with a `ghe-oidc` provider, whose
name is a live resource id rather than a description, and no such pool exists in that project. The first Project was
onboarded onto the existing `github` provider by hand. Reconciling the two is ENG-255's work, and it is the reason a
provider's `attributeCondition` must never be rewritten by hand: one CEL expression guards every identity in the pool,
Navigator's own `navigator-ci-pusher` deploy identity included, so a clause appended carelessly breaks Navigator's
deploys an hour later and somewhere else.

The thin caller workflow lives in the Project repository. `navigator site projects repository scaffold` writes
`.github/workflows/publish.yml`, so a scaffolded repository never hand-copies it. It grants `id-token: write`, installs
with a locked dependency graph, lints, typechecks, tests, and builds with the derived Vite base, runs the gate, then
then publishes:

```yaml
# <organization>/<project-code>/.github/workflows/publish.yml — an example of what a
# Project repository contains, not a file in this repository.
name: publish
on:
  push:
    branches: [main]
permissions:
  contents: read
  id-token: write            # required to mint the OIDC token WIF federates
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<sha>            # v7
      - run: pnpm --dir portal install --frozen-lockfile
      - run: pnpm --dir portal lint
      - run: pnpm --dir portal typecheck
      - run: pnpm --dir portal test
      - run: pnpm --dir portal build            # Vite base /app/projects/<code>/portal/
      - uses: neon-law-source-code/navigator/.github/actions/validate@YY.M.D
        with:
          version: "YY.M.D"
          project_repository: true              # the one gate: source-only, no legal files, mounted
      - uses: neon-law-source-code/navigator/.github/actions/application-publish@YY.M.D
        with:
          applications_bucket: ${{ secrets.NAVIGATOR_APPLICATIONS_BUCKET }}
          workload_identity_provider: ${{ secrets.NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER }}
          service_account: ${{ secrets.NAVIGATOR_APP_PUBLISHER_SERVICE_ACCOUNT }}
```

### The publisher's grant is prefix-conditioned, and one identity cannot serve two Projects

The applications bucket is **shared**: every Project's portal lives in it under its own `<code>/portal/` prefix, and
that prefix is derived by the action rather than enforced by Google. So the publisher's grant carries an IAM condition
naming exactly one prefix — `cli/src/devx/gcp/app_publisher.rs`, `publisher_condition_expression`. Without it, any
Project's CI could overwrite any other Project's portal, which is privileged client-facing work product Navigator serves
same-origin.

The role bound under that condition is a custom one holding exactly `storage.objects.create`, `storage.objects.get` and
`storage.objects.update`. No predefined role is create-and-update without delete: `objectCreator` is create-only, and
`objectUser` and `objectAdmin` both carry delete. It deliberately excludes `storage.objects.list`, which is evaluated
against the *bucket* — no object-name condition can scope it, and granting it would leak every other Project's object
names. The publish does not need it, because it uploads with `cp`, which never lists.

**A condition lives on a binding, and a binding names one role and one member set, so a publisher account can carry
exactly one prefix.** One publisher identity per Project therefore follows from the shape rather than from preference: a
second Project bound to the same account either widens the condition until the isolation is gone, or needs its own
account. So the account id carries the Project code, and `--applications-publisher-repo` is repeatable, once per
Project. The applications organization stays singular: one Workload Identity provider serves the deployment and its
attribute condition names exactly one `repository_owner`.

A provisioning run that finds a publisher already bound to a *different* prefix still **refuses** rather than repointing
it. With one account per Project that is reachable only after a repository rename — it used to be what rolling out a
second Project hit, because the account was derived from the GCP project id alone and every Project shared it. The live
policy cannot distinguish a rename, where repointing is correct, from a collision, where repointing silently revokes
another Project's publish, so the ambiguity is surfaced to an operator instead of resolved by guessing. After a genuine
rename, remove the stale binding by hand and re-run.

#### The account id is `nav-pub-<code>`, and a Project code longer than 22 characters is refused

A GCP service-account id is 6-30 characters, so the code and its prefix share a 30-character budget. `nav-pub-` spends
eight of them and leaves twenty-two for the code, which is asserted in `cli/src/devx/gcp/app_publisher.rs` as
`PUBLISHER_CODE_MAX_LEN` rather than written down twice. The prefix reads *whose*, *what*, *which Project*, matching the
owner-first order of the sibling `navigator-web`, `navigator-drive` and `navigator-ci-pusher` accounts; it is an
instance of `nav-<role>-<code>` rather than a name, so a second kind of per-Project identity gets a slot in the same
scheme; and because the prefix alone is eight characters and begins with a letter, every id the scheme can produce
satisfies GCP's minimum length and its leading-letter rule without a branch to check them.

**The Project code travels verbatim, and a code that does not fit is refused before anything is provisioned.** The three
alternatives all fail on trust rather than on length. Truncating collides silently: two codes sharing their first
twenty-two bytes fold onto one account, and the next run repoints the first Project's conditioned binding — the outcome
the refusal above exists to prevent, reached by a path that never consults it. A hash is unauditable, because the id is
the only human-readable link between a principal in a bucket IAM policy and a portal prefix. An ordinal would put an
account's meaning in the order of a configuration list, so an edit to that list would silently repoint every account
after it.

No prefix makes the ceiling go away, which is why the prefix optimizes for legibility instead. Codes are client-derived,
and a `surname-mattertype-jurisdiction` shape reaches twenty-nine characters — over the limit even with a zero-length
prefix. So the practical rule is a property of a Project's code, not of this module: **a Project whose portal publishes
needs a code of at most 22 characters.** The refusal names the code, the id it would have produced, both limits, and the
fact that the code is also the repository name and the bucket prefix, so it is changed once at Project creation or not
at all.

### The `neon-law-staging` sample lane

Three public repositories — `neon-law-staging/sample-litigation`, `/sample-transactional` and `/sample-estate` — each
hold one sample portal, named for the Project code it mounts on. Because the repository name *is* the code, the action's
derived prefix is already correct and no `repository:` override is needed. `dist_dir: dist` is set because these
applications live at the repository root, so the build emits `dist/` rather than `portal/dist/`.

The caller workflow for them is `docs/examples/sample-portal-publish.yml`, copied into each repository as
`.github/workflows/publish.yml`. **It is applied and live**: `neon-law-stg` carries a `nav-pub-<code>` publisher
identity for each of the three, and every `publish` run since it went in on 2026-08-26 has completed successfully. The
two credential coordinates are read from repository *variables*, not secrets — a Workload Identity provider resource
name and a service-account email are public identifiers with no key behind them, so GitHub's per-repository OIDC
condition and the bucket's IAM prefix condition are the actual trust boundary, not the secrecy of these two strings. The
bucket and object prefix are not passed in at all; each repository derives them from its own `navigator.yaml` through a
checked-in `.github/navigator.py`, which also backs the origin gate (`.github/no-external-references.py`) run in the
same job.

**The origin gate is a copied file, so its parsing rule is written down rather than left to the copy.** Neither script
is written by `navigator projects repository scaffold`; both were added by hand, and a new Project repository gets them
by copying. Copy from one of the three sample repositories above, which hold only synthetic source and carry the
corrected parser — not from an arbitrary Project repository, where the copy may predate the correction.

`navigator.py` parses `navigator.yaml` with a small hand-written parser, deliberately not a YAML library: a gate whose
job is to be able to say no cannot depend on `pip install` succeeding. Inside `allowed_hosts` and `allowed_prefixes`, a
`key: value` line **splits on the first `": "` and never on the first `":"`**. That is YAML's own rule, and it is the
only reading under which a URL can be a key.

Splitting on a bare `":"` makes the key of `https://example.test/x: reason` the string `https`. The gate's only consumer
of that map tests `full.startswith(prefix)`, so a bare scheme is a prefix of every `https://` reference in the bundle
and nothing is ever reported. **The gate does not go red or crash — it prints `no external references: N built file(s)
reference no host but our own` and exits 0**, which is an affirmative claim that the bundle is clean and therefore ends
the investigation rather than starting one.

Two properties make this worth pinning rather than simply fixing:

- **`allowed_hosts` escapes by luck, not by design.** A hostname has no colon before its `": "`, so the wrong split
  happens to yield the right key. A host written with a port re-enters the defect through the door that looks safe.
- **The gate's failure message solicits the entry that disables it.** A violation prints "add it to `allowed_hosts` or
  `allowed_prefixes` in navigator.yaml with the reason". Under the bare-colon split, a single entry naming one host
  suppresses reporting for *every* host, so the documented remedy for a failure is the action that turns the control off
  — and the check goes green, which reads as the problem being solved. The failure mode gets worse the more carefully an
  engineer follows the tool's own guidance.

A copy of `navigator.py` should therefore carry the parser's cases and a `--test` entry point that runs them, so the
rule is enforced in the repository that depends on it rather than remembered.

**Upload order is load-bearing, and the never-delete rule is what distinguishes a private, shared applications bucket
from a public marketing site.** The action uploads in two passes — everything except `index.html` first, then
`index.html` last — so that by the time any HTML naming a new hashed filename is readable, that file already exists.
Neither pass deletes: a stale hashed asset is left unreachable rather than removed, and one Project's publish can never
prune another's objects out of the flat namespace. It then stamps `index.html` with the publish provenance — commit,
build time, and repository metadata — as GCS custom metadata surfaced at `x-goog-meta-commit` and its siblings.

**Rollback is a revert on `main`, republished.** There is no rollback job. A bad bundle is undone by reverting it on the
Project repository's `main` and letting the caller workflow publish the reverted tree; because the action never deletes,
every rollback is a forward publish rather than a recovery of something removed.

The versioned reusable-workflow home for the shared caller is `ux/core`; wiring the thin caller there — so a Project
repository consumes one `uses:` line instead of transcribing the job above — is a hand-off, because this repository
cannot push to `ux/core`.

## Scaffolding a repository

```bash
navigator site projects repository scaffold <project-code> --dir . --action-version YY.M.D
navigator site projects repository validate .
```

`scaffold` is idempotent and leaves existing files alone. It writes the repository shell and the templates half — the
gate workflow, `README.md`, `AGENTS.md`, `CLAUDE.md`, a `templates/project_template.md` placeholder, and `tests/`.

The generated gate pins Navigator's validate action to `--action-version`, which defaults to the release the running
`navigator` reports as its own version — but only when this binary can actually vouch for that version: a downloaded
release binary, or one built with `NAVIGATOR_RELEASE_TAG` set, both of which can only report a version this repository
has already published. A plain local build cannot make that promise (`[workspace.package].version` is bumped on `main`
days before the matching tag exists), so it carries no default at all, and `--action-version` must be named explicitly.
A value that is not an exact release tag — including no value, when this binary cannot vouch for one — is refused before
any file is written, so a gate that could never resolve is never created.

It does **not** write `portal/`. That arrives from the vibe-coding lane ([`vibe-coding`](vibe-coding.md)), which knows
how to make a Vite application and which released `@neon-law/ux` version to pin. Keeping it out of the scaffold is what
lets `validate` be unambiguous: `portal/` present means there is a portal to hold to the Vite contract, and absent means
this Project does not have one yet.

`validate` accepts all three shapes — templates only, a portal only, or both — and reports a repository carrying neither
distinctly rather than failing it. A Project may legitimately open before either half exists.

The template directory is flat. Each `templates/<code>.md` file is a Project-local notation blueprint; it is not part of
Navigator's shared `templates/neon_law` or `templates/forms` catalog. Navigator reads the file at `main`, validates its
notation contract, persists its bytes as a content-addressed Asset, and records the imported commit SHA as provenance.

## Opening a Project from the browser, end to end

The mechanisms above are documented separately because each is owned by a different piece of code. A lawyer or operator
experiences them as one sequence, not five, so this section threads them together in order.

1. **Open the matter.** A lawyer-tier account fills the form at `/app/projects/new` — name, code stem, Entity,
   description, scope of services, client DRI, and the conflict-check attestation. Submitting calls
   `store::projects::open_matter` and redirects to `/app/projects/<code>`, where `<code>` is the typed stem plus
   Navigator's generated suffix — the redirect is the first and only place the final code is shown back.

2. **Provision the repository and Drive folder.** [Provisioning the three handles](#provisioning-the-three-handles)
   describes what this step does; the detail worth calling out here is that it does not happen automatically for a
   matter opened through the browser form above. `POST /app/api/projects`, the CLI's own open command, and the MCP tool
   all call `store::project_surfaces::reconcile_after_open` as part of opening — the browser form's handler does not, so
   a matter opened purely through step 1 leaves `repository_url` and the Drive folder unset. Run the reconciler
   explicitly to create (or adopt) the empty private repository and Drive folder:

   ```bash
   navigator site projects surfaces reconcile --project <code>
   ```

3. **Populate the repository.** Clone it, then run [`scaffold`](#scaffolding-a-repository):

   ```bash
   navigator site projects repository scaffold <code> --dir . --action-version <YY.M.D>
   ```

   Commit and push what it writes — that push is what makes `.github/workflows/gate.yml` live on the new repository.

4. **Build a portal, if this Project needs one.** A separate, later decision made in the `vibe-react` lane against a
   pinned `@neon-law/ux` release; `scaffold` deliberately does not write `portal/`.

5. **Wire the publish secrets** described in [Publishing the built bundle](#publishing-the-built-bundle) — one
   publisher identity per Project, provisioned by `cli/src/devx/gcp/app_publisher.rs`.

6. **Verify.**

   ```bash
   navigator site projects doctor --project <code>
   navigator site projects drift --dir ~/<organization>
   ```

Step 2's gap is current behavior, not a documented design choice: nothing marks the browser-only path as intentionally
thinner than the API, CLI, and MCP paths that provision synchronously, so treat it as something to check for rather than
something the form guarantees.

## Local checkouts

One checkout root per organization, holding one directory per Project code. The root is the organization's own name, so
nothing has to be translated between the coordinate and the path:

```text
~/neon-law/<project-code>
```

These are **source** roots. Git never stores legal files, so they must not converge with the optional Drive mount
(`NAVIGATOR_PROJECTS_DRIVE_MOUNT`), which is a workstation view of the ingest dropbox, not the working-file store.

## Verifying a machine

`navigator site projects doctor` reports whether this machine and one Project workspace actually satisfy the map above,
before anything is created:

```bash
navigator site projects doctor
navigator site projects doctor --project acme
```

It resolves the active deployment from `NAVIGATOR_GCP_PROJECT_ID`, then reports that deployment's Google Workspace,
Shared Drive, and Projects root folder, an optional local Drive mount, the stored site login, and — with `--project` —
that Project's Drive folder path, its one repository coordinate, and the path its portal mounts at.

The command is strictly read-only, and it now makes no network or database call at all: the diagnosis is a pure function
of an environment lookup, a filesystem-existence probe, the stored credentials, and a clock. A Workspace, Drive, folder,
or identity mismatch exits nonzero rather than warning. Configuration that is genuinely optional, such as an unset Drive
mount or an absent login, is reported as a warning and does not fail the run. A deployment that cannot be resolved stops
the report immediately, because every later coordinate would otherwise describe some other Workspace.

It is not `ops doctor`, which diagnoses scheduled-job health in a running Kubernetes namespace.

To create or adopt the three handles after a failed or legacy open:

```bash
navigator site projects surfaces reconcile --project acme
```

The same pass runs best-effort when a matter opens. This command is the operator retry; it talks to Drive and the forge
when those services are configured, and skips a surface when they are not.

## Reconciling repositories against live rows

One `projects.code` names both a repository and a row, and nothing makes the two agree. A repository declares its code
in `navigator.yaml`; a row records its repository in `project.repository_url`. Either side can be written without the
other, and neither side complains.

Reconciliation is therefore two halves, in two places, because the two questions need different things:

| Question | Answered by | Needs |
| --- | --- | --- |
| Does this row agree with the repository it records? | [the reconciliation door][door] | One row and a rule |
| Does this repository's declared code name a live row? | `navigator site projects drift` | The checkouts on a machine |

[door]: #reconciling-the-rows-against-what-they-record

The row half belongs on the server because a row can be checked against itself: a Project code *is* its repository name,
so a row whose URL names a different repository is drift with no checkout involved. The repository half cannot be: only
a machine holding the clones knows what repositories exist. This section is the second half.

```bash
navigator site projects drift --dir ~/<organization>
navigator site projects drift --dir ~/<organization> --all --json
```

It reads every checkout directly under `--dir` — the [local checkout root](#local-checkouts) — and takes the live
Project codes from `project_codes` on the reconciliation report. The codes rather than the findings, because a row that
is entirely fine produces no finding: an absence in the finding set would mean "reconciled" and "does not exist"
indistinguishably, and the whole question here is which codes exist.

Deliberately not `GET /app/api/projects`. That route returns the *caller's* matters — `store::access::visible_projects`
scopes to participation rows for every firm tier, Owner and Admin included — so a repository whose row exists but the
caller does not participate in would read as a repository with no row at all, which is the loudest finding this command
emits. The reconciliation door is admin-tier and reads every row.

| Finding | Meaning | Severity |
| --- | --- | --- |
| `repository-has-no-row` | Declares a code no live row carries; a portal under `<code>/portal/` mounts nowhere | fail |
| `duplicate-code` | Two checkouts claim one Project code. | fail |
| `unreadable-manifest` | `navigator.yaml` is present but unparsable, or names an invalid code. | fail |
| `manifest-disagrees-with-name` | Declares a code other than the repository's name. | warn |
| `no-manifest` | The checkout declares no Project, so it cannot be reconciled. | warn |
| `rowless-by-declaration` | The repository declares it is meant to have no row. | counted, never failed |

Warnings do not make a fleet drifted, in the same sense `projects doctor` uses. It is strictly read-only on both sides:
it creates no row, patches none, and closes none — reconciling a repository to a row is a decision about a matter.

Exit codes are three values rather than two, because a gate reads them:

| Code | Meaning |
| --- | --- |
| `0` | Every repository reconciles. Warnings do not change this. |
| `1` | At least one failing finding. |
| `2` | The report could not be produced — the scan root, the login, the host, or the response. |

The split between `1` and `2` is the one that matters: a gate treating "drifted" and "could not ask" alike goes green on
an expired token. `--json` carries the same answer as `reconciled`, and each finding serializes with its own fields
beside its `kind` and `severity`, so a consumer reads the repository or code it needs by name rather than parsing the
`detail` sentence.

### A repository declares its own absence

Not every repository without a row is drift. A closed matter may deliberately carry no row, and a command that reports
known-good repositories as failures is a command nobody runs twice. The repository says so in the manifest it already
carries:

```yaml
# navigator.yaml
project: <project-code>
no_live_row: the matter closed in <month>; no row was opened
```

The value is the reason, not a boolean, because a boolean records that someone silenced a finding without recording why.
`no_live_row: true` is refused as a manifest error rather than honored as a suppression.

The suppression cannot live in Navigator's own source. A Project code *is* a client identifier — it names who retained
the firm — and this repository is public, so a constant list of the codes to skip would publish exactly what
[`AGENTS.md`](../AGENTS.md) forbids. A `--ignore` flag only moves that list into a runbook or a CI invocation, where it
is written down just the same and reviewed less. Inferring intent from shape — no `portal/`, no `seeds/`, so nothing is
load-bearing — is worse than either: an empty repository is also what a brand-new *unreconciled* Project looks like, so
that rule would go quiet about precisely the gaps the command exists to find.

This is the opposite of the call the allowed-root list in `cli::projects::repository` makes, and deliberately so. That
list governs a rule identical for every repository — which paths may sit at a root — so centralizing it costs nothing
and keeps the gate from going advisory. Whether one matter is meant to have a row is a per-matter fact only that matter
knows.

Declared row-less repositories are still counted in the footer, and `--all` lists them. Suppressed and silent are
different things: a report that hides repositories without saying so fails the same way as one that cries wolf.

## Shared notations

The notations that are not specific to one Project live in **this** repository, under `templates/neon_law/<product>/`
and `templates/forms/`. They are Navigator's own catalog, versioned and validated with its source.

A Project repository's `templates/` directory is separate from that catalog rather than an extension of it: it carries
the blueprints belonging to that Project, and Navigator imports each one at the commit it reads. A Project-local
notation may record its lineage in a `derived_from` frontmatter field, which is documentation — the rule engine does not
resolve it, so it creates no dependency on any other repository being present.

**There is no cross-deployment template repository, and a Project repository must not assume one.** Everything a
Project's notations need is either in that Project's own `templates/` or in this repository's catalog.

## Source boundaries

`neon-law-source-code/navigator` is Navigator's source repository. It is not a Project repository. Each Project's
repository is its own deployment-specific source repository, and its portal pins a released shared component-library
version.

A Project repository may hold notation templates, portal source, fixtures, tests, and the checked-in configuration
required to build or validate them. It may not hold client uploads, answers, generated legal documents, secrets,
dependencies, or build output.

## Access boundary

Navigator Project participation authorizes Navigator and the served client portal. It never grants source-forge access.
Outside lawyers work through Navigator, Drive, and the served portal without any membership of the forge that hosts the
source. Repository access is an independently administered source-control decision.

This is also why the model stops at one repository per Project rather than one repository per organization with
`projects/<code>/` subdirectories, which would be the same logic one step further. **Repository access is the per-matter
access boundary**: a per-organization monorepo would hand every contributor every matter's source. One repository per
Project code is the floor.

## Implementation boundary

This contract deliberately leaves deployment provisioning, access reconciliation, and migration to their own
implementations. Those changes must preserve these authorities and may not reintroduce legal files into Git.
