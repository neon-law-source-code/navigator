# Project workspace and repository contract

Each Navigator [Project](glossary.md#project) coordinates four distinct surfaces. They are not interchangeable stores:

| Surface | Authority | Contains |
| --- | --- | --- |
| Google Drive | Firm working files | Legal files and internal working material |
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

The Project code is the stable Navigator `projects.code`. It is the Project folder basename in its deployment's selected
Drive root. That equality is why the slug rules are what they are: lowercase letters, digits, and single hyphens,
alphanumeric at both ends, at most 80 characters. Drive and macOS are case-insensitive, so uppercase would let one
folder answer to two codes; one separator keeps the mapping an equality check rather than a normalization. The code does
**not** name the repository.

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
public identity: its show page, its client portal, its repository name, and its folder in the firm's shared drive are
all that one word, and Navigator never invents it.

```text
/app/projects/<project-code>            the matter's show page — never its internal UUID
/app/projects/<project-code>/portal/    the client portal this repository publishes
```

**The repository name is the code, and today it is what every publish path reads.** `cli/src/projects/repository.rs`
takes it from the checkout directory, `.github/actions/application-publish` takes it from
`github.event.repository.name`, and Vite derives its base from the checkout directory too (`basename(resolve(__dirname,
'..'))`). The gate re-derives it and refuses a name that is not a valid code, so a checkout cloned into a differently
named directory fails there rather than publishing under the wrong prefix.

**A repository may also declare its Project in a root manifest, and that manifest is part of the layout.** Both
spellings are live and both are allowed roots: Project repositories carry `navigator.yaml`, and a sample-project bundle
carries `navigator.yml` (`store::sample_project::MANIFEST_FILE`), which `store::sample_project::project_code_for` reads
and refuses when the declared code is not the one the bundle is being published under.

So the code is derived in one place and declared in another, and nothing makes the two agree. Every repository shipping
today keeps them aligned by convention: each sample repository is named for the code it mounts on, so
`neon-law-staging/sample-litigation` publishes as `sample-litigation` and the publish action needs no override. That is
a naming discipline, not an enforced rule — name a repository anything else and the derived prefix silently stops
matching the declared code. Collapsing this to one filename, one key, and one reader — and deciding whether
`application-publish` should read the manifest rather than the repository name — is an open decision, not settled here.
Until it lands, refusing either spelling would fail a repository that is correct as shipped, so the layout admits both.

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

## The CI gate

One composite action is the whole gate, consumed identically by every Project repository in every organization:

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

**There is no path filter, and that is deliberate.** A filtered job that skips reports success for work it never did,
and a required check a skip can satisfy is not a gate. So the one job always runs and each half no-ops over a repository
that does not carry it. The job is spelled `ci`, which is the one required context `navigator ops github setup` binds.

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
Navigator streams object-by-object. Objects land **flat** under that prefix; the action derives `<code>` from
`github.event.repository.name`, exactly as the gate derives it from the checkout directory, so the object prefix cannot
disagree with the served mount wherever the repository is hosted.

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

The thin caller workflow lives in the Project repository, not here. It grants `id-token: write`, installs with a locked
dependency graph, lints, typechecks, tests, and builds with the derived Vite base, runs the gate, then publishes:

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

The caller workflow for them is `docs/examples/sample-portal-publish.yml`, ready to copy unchanged into each repository
as `.github/workflows/publish.yml`. **It is not yet applied.** The credential needed to push a `.github/workflows/`
change is available, so that is no longer what blocks it; what blocks it is that `neon-law-stg` carries no publisher
identity yet, and applying the workflow before it does would leave three public repositories with a workflow that fails
at authentication on every merge to `main`.

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
navigator projects repository scaffold <project-code> --dir . --action-version YY.M.D
navigator projects repository validate .
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

## Local checkouts

One checkout root per organization, holding one directory per Project code. The root is the organization's own name, so
nothing has to be translated between the coordinate and the path:

```text
~/neon-law/<project-code>
```

These are **source** roots. Git never stores legal files, so they must not converge with the Drive mount
(`NAVIGATOR_PROJECTS_DRIVE_MOUNT`), which is a separate path holding the firm's working files.

## Verifying a machine

`navigator projects doctor` reports whether this machine and one Project workspace actually satisfy the map above,
before anything is created:

```bash
navigator projects doctor
navigator projects doctor --project acme
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

## Reconciling repositories against live rows

One `projects.code` names both a repository and a row, and nothing makes the two agree. A repository declares its code
in `navigator.yaml`; a row records its repository in `project.repository_url`. Either side can be written without the
other, and neither side complains. `navigator projects drift` reads both and reports where they disagree:

```bash
navigator projects drift --dir ~/<organization>
navigator projects drift --dir ~/<organization> --all --json
```

It reads every checkout directly under `--dir` — the [local checkout root](#local-checkouts) — and lists the live rows
from `GET /app/api/projects`. That route rather than `GET /app/projects.csv`, which `site projects list` uses: the CSV
carries `id, code, name, status, entity_name` and no `repository_url`, so it can answer only whether a repository has a
row, never whether a row has a repository.

Both directions matter, and the reverse one is the direction nothing checked. A row's `repository_url` is optional, so a
row that never recorded one is indistinguishable at a glance from a row that did — and a repository is resolved back to
its row *through* that column, so an empty one fails every resolution silently. A recorded-but-stale URL is quieter
still: a forge rename leaves a redirect, so the old URL keeps resolving over HTTP long after the repository stopped
answering to that name.

| Finding | Meaning |
| --- | --- |
| `repository-has-no-row` | Declares a code no live row carries; a portal under `<code>/portal/` mounts nowhere. |
| `row-has-no-repository-url` | The row records no repository at all, so nothing resolves a repository back to it. |
| `row-repository-absent` | The row's URL names a repository not present under `--dir`. |
| `code-mismatch` | The row and the repository its URL names spell the code differently. |
| `manifest-disagrees-with-name` | Declares a code other than the repository's name. Warned, not failed. |
| `duplicate-code` | Two checkouts claim one Project code. |
| `unreadable-manifest` | `navigator.yaml` is present but unparsable, or names an invalid code. |
| `no-manifest` | The checkout declares no Project, so it cannot be reconciled. Warned, not failed. |
| `rowless-by-declaration` | The repository declares it is meant to have no row. Counted, never failed. |

The row-side findings assume `--dir` holds the whole fleet. Run against part of it, "no repository is present" is true
but uninteresting, so each such finding names the directory it searched rather than hiding the assumption behind a flag.

Warnings do not make a fleet drifted, in the same sense `projects doctor` uses. The command exits nonzero only when a
finding is a failure, and it is strictly read-only on both sides: it creates no row, patches none, and closes none.

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
