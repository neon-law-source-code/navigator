# Access model — role + participation

Neon Law Navigator separates **what a person is** (system-wide tier) from **what a person sees** (per-project scope).
Both answers live in the database and flow into embedded Rego; neither lives in the IdP token, which supplies only
identity (`sub`, `email`).

> **Role decides the tier; participation decides the scope.**

The two columns:

| Column | Table | Decides |
| --- | --- | --- |
| `role` | `person` | The tier: `owner`, `admin`, `lawyer`, `clerk`, or `client`. Anonymous = no row. |
| `participation` | `person_project_role` | Which side of a Project a person is on. Derived from `role`. |

The two columns are independent. A Clerk who is *also* a client of the firm for their own LLC carries the Clerk role on
the person row (their firm work) and a `person_project_role` row on their personal matter with the client participation.
The system answers "what can this person do" by reading both.

## The five stored tiers

`person.role` is a `string` field with `ASSERT $value IN ['owner','admin','lawyer','clerk','client']`. Rust models it as
a plain enum. The authority order is `owner > admin > lawyer > clerk > client`.

### `owner`

The human accountable for and in control of the deployed system. Owner is the highest tier and inherits every Admin and
Lawyer capability. `/app/projects/{code}` still requires a firm-side `person_project_role` row before it renders the
full workbench — Owner included: a matter nobody put them on renders only the participation ledger (add, edit, or remove
who is assigned), never that matter's documents, notations, or other content. `/app/projects` itself is unscoped for
Owner — it lists every matter in the deployment, the same administrative-listing shape a reconciliation report already
reads for its own deployment-wide question — which is what gives the detail page's participation-only carve-out
somewhere to navigate from. Only an Owner may create, edit, or demote an Owner identity; Admin cannot govern the tier
above it. Person deletion remains client-only, so no privileged identity is deletable through that command.

### `client`

A person the firm represents on at least one matter. The client lens sees only projects where the person is recorded as
the client-side participant — a `person_project_role` row whose participation is client-side, which includes the row
carrying the `is_client_dri` accountability marker. Client matter access is portal-native: documents,
Engagements/Notations, invoices, and reviewed artifacts are rendered through `web` after the Project visibility check.
Clients do not receive Git clone URLs, Git PATs, branch names, commit SHAs, or direct GCS bucket credentials.

### `clerk`

A supervised **non-lawyer** firm worker. Clerk enters only the dedicated `/clerk` coordination surface, never `/lawyer`,
MCP, A2A, Git, person administration, legal drafting, approval, or advice. `/clerk` is read-only and lists only Projects
where the Clerk has firm-side participation and one of the matter's `is_lawyer_dri` rows names a licensed `lawyer`,
`admin`, or `owner` lawyer. It shows the matter's name, status, supervising lawyer, and a link to the Project's client
portal—but no firm documents, legal work, or write control on this surface. The portal is participation-gated
identically for every viewer (`store::access::can_see_project`, which is `matter_viewer(...).is_some()`), so admitting a
Clerk to it grants nothing the matter page's own visibility does not.

On an assigned matter, the `View as Client` control starts the established client-lens session for that matter's client
DRI and takes the Clerk to the client rendering. The impersonation banner names the effective client and provides the
single exit back to the Clerk session. The Clerk page itself remains read-only; once in the client lens, every client
read or action continues through its ordinary client-side authorization. The server resolves both the current Clerk's
supervised access and the matter's client DRI rather than accepting either identity from the browser.

A lawyer already participating in the matter controls this directly, by adding or removing the Clerk's participation row
through `POST` / `DELETE /app/api/projects/{id}/participants[/{role_id}]` — see [Participation](#participation) below.
There is no separate visibility flag: the participation row *is* the toggle, on while the row exists (and the matter
still names a licensed lawyer DRI), off the moment it is removed.

"Never `/lawyer`" now holds without exception. The firm brand fonts, the one object that used to need one, moved to `GET
/app/team/fonts/gorp-serif.zip`, where the team home's own prefix rules admit every firm tier — a brand asset is not
lawyer work, and the path now says so.

### `lawyer`

A **licensed lawyer** authorized to perform legal work through Navigator, regardless of employer or email domain. The
Lawyer lens sees only projects where the lawyer has a firm-side `person_project_role` participation row.

Lawyers may also be clients on their own matters, but that is a separate client-lens fact: the matter surface at
`/app/projects` renders each caller through their own lens, while `/lawyer` shows the matters they work on for the firm.

Designating the lawyer as a lawyer DRI is not a separate access grant: the `is_lawyer_dri` marker rides that same
participation row, so a DRI is a matter person by construction. A matter's `is_lawyer_dri` rows are its disclosed lawyer
DRIs; none of them may be a Clerk.

### `admin`

A **licensed lawyer** with system-administration authority — manage the person table, rotate keys, archive projects.
Admin is a superset of Lawyer. Like Owner, a matter nobody has put an Admin on still gates its full content: the
workbench, documents, and notations at `/app/projects/{code}` stay behind the participation row every tier needs. What
Admin sees instead of a `404` there is a participation-only rendering — enough to see the matter and staff it, nothing
it discloses beyond that — and `/app/projects` lists every matter so there is something to navigate to. Privileged reach
is a surface you navigate to rather than an invisible widening of a shared route, which is what makes a lens bug
distinguishable from an intended bypass — the two are otherwise indistinguishable from a response body. Admin cannot
create, edit, or demote an Owner.

### *anonymous*

No row in `person` at all. Sees the host's own public pages and the login door, and nothing else. Nearly every page on
the firm's host is anonymous, including the [presentations](glossary.md#presentation) catalog at `/presentations`, every
talk beneath it, and the `/workshops` catalog and workshop material.

Every shared Navigator surface — `/app`, `/lawyer`, `/admin`, `/clerk`, the JSON API, `/templates/*`, `/app/api`, and
`/app/api/openapi.json` — composes behind one router-level boundary, `portal::auth::require_session`. An anonymous
browser is sent to `/auth/login?return_to=…`; an anonymous machine caller gets a `401` with a structured
`{"error":"unauthenticated"}` document. Default-deny is therefore a property of router composition, not of a Rego rule
that would have to redeploy in lockstep with the binary. Embedded Rego still runs behind boundary and decides *which*
authenticated caller may proceed.

The anonymous allowlist is explicit, small, and pinned by `portal/tests/router_contract.rs`:

- the OIDC login, callback, and logout routes, plus the `navigator` CLI login handshakes under `/auth/cli/*`;
- the static assets under `/public/` that the login page renders;
- `/assets/*`, which reads only the deployment's dedicated private marketing-assets bucket through the GKE workload
  identity; client documents, exports, and logs have no corresponding anonymous route;
- the `/health` and `/readyz` probes and the `/version` deploy-identity probe;
- webhook ingress whose sender authenticates by signature or path secret — SendGrid inbound mail and delivery events,
  and the e-signature completion callback. The GitHub webhook receiver is not on this list: it lives on
  `workflows-service`, a separate host, and `web` answers `404` for it
  (`portal/tests/router_contract.rs::web_does_not_serve_the_github_webhook_receiver`);
- the DocuSign consent callback, the provider's return leg of an admin-initiated consent grant;
- the two contributor reference surfaces, `/design` and the workspace documentation at `/docs` and `/docs/{slug}`. Both
  render their own `200` for a reader with no account rather than answering the login door, and both carry
  `inject_optional_session` so a signed-in reader still gets the authenticated nav. The documentation is anonymous
  because the repository is source-available: those documents are the manual for software anyone can clone, so a login
  door in front of them guarded nothing. `/app/docs` is a second door to the same index wearing the application chrome,
  and it stays gated — what it restricts is that surface, not the documents.

The A2A agent card is *not* on that list. The whole API surface, its documentation, and the card itself live under the
private `/app/api` prefix and require a session, so A2A discovery is not self-service: a client cannot read the card to
learn which credential to present, because reading it already requires one. Gemini Enterprise takes its OAuth details
from the registration form rather than from spec-driven discovery, so the client this serves is unaffected; onboarding a
standards-based A2A client means handing over the OAuth details out of band.

`owner` and `admin` are supersets of `lawyer`, not separate axes. A system Owner or administrator who can exercise this
application's legal-work and project bypasses must be a lawyer; a non-lawyer operations worker belongs in `clerk` and
needs only the separate supervised capabilities explicitly granted to that role.

## How a `person` row is created

Signing in with the IdP does not, by itself, create a `person` row. The OAuth callback resolves the IdP-authenticated
subject against the table (`portal::oauth::resolve_person_from_claims`):

- an existing row (matched on `oidc_subject` or, case-insensitively, `email`) signs in with its stored role;
- the configured `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` is JIT-created as `owner` on first login (the carve-out that keeps a
  fresh deploy from locking its Owner out), and role-healed back to `owner` on every subsequent login;
- **every other unknown email is refused with a `403`** — onboarding is operator-mediated by default.

### Self-signup (global toggle, default off)

`NAVIGATOR_SELF_SIGNUP_ENABLED` is a deployment-wide capability that is **off unless explicitly set** (affirmative
values: `1`, `true`, `yes`, `on`). Off is byte-for-byte the `403` behavior above. When on, the first login for an
unknown verified email JIT-creates a `client` with **no `person_project_role` rows** — an empty portfolio until an admin
assigns participation. Embedded Rego and the role/participation model are untouched; self-signup only changes whether an
unknown email becomes a scopeless `client` or a `403`. The bootstrap-Owner carve-out is independent of this toggle. A
training deployment turns this on when trainings open; production keeps it off. See #738.

## Concrete people in the seed data

- **Nick** (`nick@neonlaw.com`, lowercase) — the primary administrator and lawyer. Role `admin`; sees every project. The
  lowercase spelling is exact: `store::seed::require_firm_domain` rejects mixed-case owner/admin/clerk seeds at load
  time.
- **Lawyers** — role `lawyer`. An outside lawyer may use their own email domain and receives only the Projects where
  they have firm-side participation; Lawyer does not confer source-forge access.
- **Clerks** — supervised non-lawyer firm workers, role `clerk`, also using lowercase `*@neonlaw.com` emails in
  canonical seed data. They are not a member of the Lawyer tier.
- **Clients** — any seeded non-firm person, role `client`. Email is the client's real address; no domain restriction.
  `client@neonlaw.com` in the KIND fixture below is the sole local exception to that non-firm-address rule.
- **The KIND-only Rauthy fixture** seeds one account per role, each with password `password`: `owner@neonlaw.com`,
  `admin@neonlaw.com`, `lawyer@neonlaw.com`, `clerk@neonlaw.com`, and `client@neonlaw.com` (per
  [`AGENTS.md`](../AGENTS.md#authentication-and-lawyer-access)). Four of the five are seeded onto one demo matter,
  *Cruller v. Prine* (project code `sample-litigation`), so each can be exercised on the same project.
  `admin@neonlaw.com` deliberately holds **no** participation on it: the fixture Admin demonstrates the ENG-81 decision
  — `/app/projects` still lists the matter (Admin's list is unscoped) and `/app/projects/{code}` still gates its content
  behind the row, rendering only the participation ledger until the fixture Admin grants themself one.
  `lawyer@neonlaw.com` carries exactly one role, lawyer, not admin.

Email identifies exactly one person regardless of casing. `person.email_lower`, a computed field, carries a unique index
(`person_email_lower`), and every lookup keyed on email goes through `store::persons::find_by_email_ci`, so an identity
provider presenting `Attorney@Example.com` resolves to the row stored as `attorney@example.com` and authorizes against
the same role. Two rows differing only by case cannot exist.

## Participation

`person_project_role.participation` records which side of a matter a person is on. It is **derived, never entered**:
`store::projects::participation_for_role` maps `person.role` onto it, so the column holds exactly one of `owner`,
`admin`, `lawyer`, `clerk`, or `client`, and a `client` is the only value on the client side.

All three write doors go through `store::participation::add_participant` / `update_participant`, and none of them takes
a participation: the lawyer matter-people form, `POST /app/api/projects/{id}/participants`, and
`aida_link_person_project` each name a person and nothing else. A `participation` sent to any of them is surplus and
unread.

This is why there is no separate vocabulary. A matter-side word that could disagree with the tier was a way to get the
access decision wrong — a `client` recorded as `attorney` is a firm-side row, which is the matter's own client reading
`/lawyer`. The kinds that used to need their own word are not participants at all:

- **`counterparty`** — an adverse party has no portal access, so it gets no `person_project_role` row. `counterparty`
  survives in `PARTICIPATION_CLIENT_SIDE` only so a legacy row keeps reading client-side; promoting an adverse party to
  the firm lens is the one direction that must never happen by omission.
- **`co_counsel`, `legal_aid_provider`** — outside counsel working the matter is a `lawyer` person. Give them the tier;
  the participation follows.

### Client-side is not the same question as client documents

The firm lens (`store::projects::can_access_as_lawyer_in_surreal`) is the exact complement of the client-side set, so
what is *not* client-side is firm-side. That complement is why `PARTICIPATION_CLIENT_SIDE` still lists `counterparty`
even though nothing writes it: dropping the word would flip any legacy adverse-party row into the lawyer workbench.
Client-side membership answers *which side of the matter is this person on*.

It does not answer *may this person reach the client's documents*, where an adverse party has no business at all. That
is `store::projects::can_access_as_client_in_surreal`: the natural-person `client`, or the client-DRI marker for a
client DRI recorded under some other party kind. Anything granting access over a matter's client document storage keys
on that predicate, never on the visibility lens.

Every row carries `inserted_at` + `updated_at` (the workspace timestamp convention). Those answer "is this still true
right now and how stale is the fact." They do **not** answer "was Libra ever an attorney on this matter." If you need
participation history, append a row to `relationship_log` — that's what the table exists for
(`store/src/schema/navigator.surql:486`).

Each person has at most one current participation row for a Project. Changing the participation updates that row; it
does not create a second, competing assignment.

Two doors write it, scoped differently on purpose. The firm-wide participation form (`/app/projects/{code}/people`)
stays admin-only, because it reads the whole people directory to pick who to add — a broad capability that changing who
can see the matter deserves. `POST/PATCH/DELETE /app/api/projects/{id}/participants[/…]` is narrower in what it reads
(it names a person already known to the caller) and admits any lawyer already participating in that matter, not only
admin — the same "must already be on the matter" re-check `/close` uses, with Owner/Admin bypassing it as everywhere
else. This is the door a participating lawyer uses to grant or revoke a Clerk's portal visibility on their own matter:
adding the Clerk's participation row is what admits them through `store::access::can_see_project`, and removing it is
the toggle back off — see [`clerk`](#clerk) above. A lawyer with no row on the matter gets the same non-disclosing `404`
an unrelated caller would, from either door.

## The directory lens

Owner and Admin need to answer "which matters exist, and who is responsible for each" without being on any of them.
Membership is not the way to answer it. A `person_project_role` row grants access to what a matter *contains*, so
inviting an Owner onto every matter to give them oversight would hand them every matter's documents, notations, and
communications as a side effect. Owner and Admin therefore receive **no participation row for oversight**, and the
matter-surface rules in the Owner and Admin sections above stand exactly as written.

What they receive instead is a narrow projection over every project — its `code`, `name`, `status`, and the people on
the matter's `is_lawyer_dri` rows. Nothing else: no notations, deadlines, documents, communications, or participation
ledger beyond that one accountable lawyer. `store::projects::matter_directory` is the whole lens, and it returns that
projection rather than a bool, which is what keeps it from being mistaken for an access check. It is admin-tier only — a
`lawyer` caller reads an empty directory, and the tier is refused before the query runs.

It is a third shape, and neither of the two predicates beside it fits:

| Predicate | Question | Answer for an admin tier |
| --- | --- | --- |
| `can_access_as_lawyer_in_surreal` | May this actor read the matter in the firm lens? | `true` — the whole matter |
| `store::access::can_see_project` | Is this person on the matter? | Only with a participation row, Owner included |
| `matter_directory` | Which matters exist, and who owns each? | The projection, holding no row on any of them |

`project.code` carries a `project_code UNIQUE` index, so the code is the stable handle the directory lists a matter
under. The lens deliberately renders no link into a matter: reaching one is membership's decision, and an Owner who
follows a link into a matter they hold no row on gets the same `404` as anyone else.

A matter with no `is_lawyer_dri` row at all reads as **unassigned** and keeps its place in the list. That is not a
degraded case being tolerated — an unaccountable matter is the single most important thing this surface exists to show,
so it renders as a flagged value rather than an error or an omission.

The surface is `/app/admin/projects`, and it is admitted by the Owner/Admin route bypass at the top of
`portal/policy/navigator.rego` — the same bypass that admits `/app/admin` itself. **It gets no Rego rule of its own.**
An `is_lawyer` rule written for this path would silently widen the firm's whole matter directory to every Lawyer person,
which is precisely the disclosure the participation ledger exists to prevent; the deny is by omission, and
`navigator_test.rego` pins it from the other side by asserting that Lawyer, Clerk, and Client are all refused.

## What `participation` is NOT

It is not the `disclosures` table. Disclosures are formal records the firm keeps about *conflicts of interest* and
*related-party relationships* — information flowing *from* the client *to* the firm about who the client is connected
to. Project membership is the opposite direction: an internal record of *who the firm has put on the matter*. The two
concepts share the same English word in casual speech ("Libra is disclosed on the Acme matter") but they're different
columns in different tables answering different questions.

If someone should see a Project's portal files, add or remove the `person_project_role` row; do not grant them GCS IAM
or expose the git repository.

If you find yourself reaching for `disclosures` to decide whether someone can see a project, stop — you want
`person_project_role`. See [glossary entry "Disclosure"](glossary.md#disclosure).

## What an External System Identity is not

`person_external_identity` (ENG-85) records the id a third-party system issues for a Person — a GitHub numeric id, a
Slack `U…`, a Google `sub` — so Navigator can name them in an outbound API call. It is an address book. **No code may
read it to make an access decision.**

That is not a scoping convenience, it is a safety property, and three separate things make it work:

| | Lives where |
| -- | -- |
| **Who** — the account id to name in the call | `person_external_identity` |
| **Credential** — what authenticates the call | firm-level service configuration, managed as secrets |
| **Authority** — whether the call should be made at all | `person.role` and the policy above |

Two rules elsewhere in the docs hold *because* the table is inert. A Clerk "never receives lawyer-work, advice, Git,
MCP, or `/lawyer` authority by inheritance", so a Clerk recorded as GitHub user `12345` gains nothing by being recorded
as such. And Project participation never grants source-forge access ([`project-repositories`](project-repositories.md)),
so this table must not become the back door that reverses it. The rule is per-system rather than per-role: a `client`
Person holding a `google` identity for Drive sharing is legitimate, and that same Person is still never provisioned into
the source forge. The schema therefore carries no blanket role constraint — enforcement belongs where provisioning
happens.

Provisioning may *resolve* a Person to an account through this table; the decision to provision anything comes from role
and policy. `cli/tests/external_identity_is_inert.rs` asserts the separation against every authorization surface by
name, so it fails in the diff that breaks it rather than the incident that reveals it.

## How embedded Rego decides

The web middleware (`portal::policy::require_policy`) evaluates an `input` document against embedded Rego on each
request:

```json
{
  "path":       ["admin", "projects", "9a..."],
  "method":     "GET",
  "session":    {
    "sub":   "<idp subject>",
    "email": "libra@example.com",
    "role":  "lawyer"
  },
  "project_id": "9a..."
}
```

`project_id` is populated by the route handler when the URL is project-scoped (`/app/projects/:code` and its document
subroutes). Routes without a project parameter leave it absent.

Embedded Rego's allow rules in priority order:

1. **Owner/Admin route bypass** — `session.role` in `{"owner", "admin"}` allows every authenticated *request*. This is a
   route-admission decision only: it says the request may reach a handler, not that the caller may see the row. On the
   matter surface the handler then applies the participation gate below, so an unassigned Owner passes embedded Rego and
   still gets a `404`. The trust call is that these tiers imply a fiduciary duty audited elsewhere (Drive activity, DB
   write logs). Operational surfaces such as `/app/admin`, `/app/admin/analytics`, and `/app/admin/people` enforce the
   Owner/Admin tier in their handlers, so the broader `/lawyer/*` lawyer-tier gate cannot expose them.
2. **Lawyer-tier surfaces** — `/app/admin/entity-types`, `/app/admin/templates`, and the other firm-internal pages gate
   on `session.role` being `"owner"`, `"admin"`, or `"lawyer"`. `"clerk"` is intentionally absent. The people directory
   is **not** among them: its browser surface is `/app/admin/people`, Owner/Admin only, since ENG-304 deleted the
   `/lawyer` mirror. The Person *commands* stay lawyer-tier at `POST/PATCH/DELETE /app/api/people*`, so what a lawyer
   lost is the form, not the capability. That tier check is the whole gate only for firm *reference* data. A `/lawyer`
   listing that reads **matter content** — `/lawyer/answers`, `/lawyer/assets`, `/lawyer/relationship-logs` —
   additionally scopes its rows to the caller's participation ledger through
   `webapp::admin_listing::require_lawyer_in_matters`, so a lawyer holding no row reads nothing there, and a row
   carrying no project link is absent from a scoped read rather than admitted. Owner and Admin keep the unscoped read.
   Two listings stay firm-wide on purpose: `/lawyer/disclosures` and `/lawyer/person-entity-roles` feed
   `store::conflicts::check_new_matter`, and ABA Model Rule 1.10 imputes a conflict firm-wide, so a lawyer must be able
   to see one arising out of a matter they are not on — scoping either would narrow the conflict check to the checker's
   own caseload. `/app/admin/letters` and `/app/admin/email-log` are Owner/Admin only: `letter` and `sent_email` carry
   no project link to scope by, so the admin gate is the interim close until one exists. Which class each listing
   belongs to is written down once, in `webapp::admin_listing::LAWYER_LISTINGS`.
3. **Clerk supervised lens** — a Clerk enters `/app/projects` with everyone else, and
   `store::access::matter_viewer` resolves them to `MatterViewer::Clerk` only when they hold a firm-side row and the
   matter has a flagged lawyer DRI who currently holds the lawyer tier. That variant renders the matter name, status,
   and supervisor. The resolver and dispatcher branch keep the supervised view conditional, and the behavior is pinned
   by test.
4. **The one matter surface** — `/app/projects` and `/app/projects/{code}/...` admit authenticated callers in embedded
   Rego, because the firm/client split is not a distinction the policy can make: it cannot read the participation
   ledger. The handler makes it, from `person.role` plus the caller's `person_project_role` row
   (`store::access::can_see_project`). A firm tier needs a firm-side row and a client needs a client-side one, so a
   `counterparty` sees the matter through the client lens and never the workbench, and a lawyer-only assignment still
   does not put the matter in a client's list. Owner and Admin are scoped the same way — there is no bypass here. Naming
   a lawyer DRI cannot grant access without a membership row, because `is_lawyer_dri` rides that membership row. The
   lawyer-only writes under this path (matter open/edit/delete, the participation forms, document upload, transcript
   intake) additionally re-check the lawyer tier in their own handlers, preserving the firm-side write boundary.
5. **API project reads** — `/app/api/projects/:id/...` allow if there is a `person_project_role` row with
   `person_id = session.person_id` and `project_id = input.project_id`. Embedded Rego does not check participation;
   action-level distinctions live in the route layer.
6. **API reads are named per resource** — there is no blanket grant on the `/app/api` prefix. The CRM directory
   (`people`, `entities`, collection and item) and the reference vocabularies (`jurisdictions`, `entity-types`) gate on
   the lawyer tier; raw Template markdown (`GET /app/api/templates/*path`) is deliberately open to any authenticated
   person, because the notation gallery at `/notations` and the Template gallery at `/templates` link it and both admit
   `client`. The API documentation at `/app/api` and `/app/api/openapi.json` admits Clerk and above.

   A `GET` route under `/app/api` with no rule of its own is denied. That is the point of naming each one: a single
   any-authenticated grant over the whole prefix would hand a `client` the firm's entire directory — the read handlers
   carry no tier check of their own — and would authorize every new read endpoint the moment it was routed. Naming each
   read fails an unnamed one closed until someone decides who should have it.

## Every API request is audited

`portal::api_audit` emits one `target: "audit"` event per request to `/app/api`, naming the acting person, their tier,
the endpoint, and the status it was answered with:

```text
User 4f2c1e08-… with role lawyer requested "/app/api/people"
```

The layer sits inside the session boundary and *outside* the policy gate, so a refused request is recorded too — a trail
that logs only successful reads is the wrong half of one. The event carries `uri().path()`, never `path_and_query()`,
and never reads the body: a `?email=` filter or a `POST` body on this surface is client content, and client content does
not belong in telemetry. Record identifiers in the path are kept deliberately, because an access log that cannot say
*which* row was read answers nothing.

Repository access is delegated to the selected forge. Navigator renders the lawyer-only repository browser link only
after the lawyer-lens Project check; the client portal receives neither that link nor forge credentials. The forge
collaborator reconciliation is a separate grant, not a substitute for Navigator's route-level authorization.

The store layer ships lens-specific helpers for the visibility query:

```rust
// store/src/access.rs
pub async fn visible_projects_as_client(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
) -> Result<Vec<Project>, String>;

pub async fn visible_projects_as_lawyer(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<Vec<Project>, String>;

pub async fn visible_projects_as_clerk(
    surreal: &SurrealDb,
    person_id: Option<Uuid>,
    role: Role,
) -> Result<Vec<Project>, String>;
```

Every project-list and project-detail handler funnels through the helper for its route lens. Both DRIs are
`is_lawyer_dri` / `is_client_dri` markers on participation rows written at matter opening, so DRI assignment lives
inside the same ledger the lens already reads rather than in a parallel column-level exception. Inlining the SQL into
individual handlers is the failure mode we are explicitly avoiding — it's how authz quietly drifts.

### Designating a DRI after the open

Each side is a **set**. A matter carries as many accountable lawyers, and as many accountable client contacts, as the
firm has put on it, so designation adds to a side and displaces nobody. Leaving a set is its own act.

`store::participation` is the one write boundary for the markers, through `DriRequest` on the add and update commands.
Every other door leaves it `Unchanged`, so linking a person never moves accountability as a side effect. Four rules hold
there, whatever the caller:

1. **The lawyer set is never empty** (`DriError::LawyerDriRequired`). The last accountable lawyer cannot step off, and
   the removal lockout defends the same invariant from the other side. Any lawyer DRI beyond the last may step off, and
   the client set may empty entirely.
2. **The tier has to be able to carry the side** (`DriError::TierMismatch`). A lawyer DRI is an accountable lawyer —
   `owner`, `admin`, or `lawyer`, never a Clerk and never a client. A client DRI is a client-side contact.
3. **The lawyer side governs itself.** Any of a matter's current lawyer DRIs may add or remove any other, themselves
   included, bounded by rule 1. A lawyer who holds no marker on that matter may not, and neither may a lawyer who is not
   on it at all (`DriError::NotPermitted`). Owner and Admin pass, which is also what designates the first lawyer DRI on
   a matter whose set is empty.
4. **The client side is the firm's call.** Designating or removing a client DRI takes `Role::is_lawyer_tier` — `owner`,
   `admin`, or `lawyer`. A client never designates their own counterpart, and a Clerk designates neither side.

Rules 3 and 4 read the actor from `DriActor` on the command, which is the same field the audit row is written from: a
rule enforced against one person and recorded against another is not an audit trail. `DriActor::System` is the trusted
internal caller — matter open, the seed, a workflow step — which is ungated and recorded with no actor.

**Every designation and every removal is audited.** `store::participation` appends a `relationship_log` entry naming the
actor, the matter, and the person moved, under `lawyer_dri_designated`, `lawyer_dri_removed`, `client_dri_designated`,
or `client_dri_removed`. The entry lands *before* the marker write: these are two writes with no transaction across
them, so an entry describing a change that did not happen is the recoverable failure, and a marker that moved with
nothing recording it is the one the trail exists to prevent.

The two surfaces differ by what they can see. The firm-wide participation form under `/app/projects/{code}/people` stays
admin-only because it reads the whole people directory; a matter's own lawyer DRIs govern their side from the workbench
at `/app/projects/{code}`, which offers add/remove on people already assigned to that matter and so needs no directory
read.

The marker matters beyond visibility: a lawyer DRI is who closes the matter. The workbench carries no close control —
closing is bespoke, asked for by email — so the accountable lawyers are who that request goes to.

The route layer carries a second gate for the lawyer project-write surface: `Role::is_lawyer_tier` is `true` for lawyer
`lawyer` and `admin`, never `clerk`. A `client` or `clerk` who reaches `/app/projects/:project_code/edit` and friends is
stopped by the handler's own tier check rather than by embedded Rego, which admits every authenticated caller onto the
collapsed matter path. A failed handler-level Project ACL returns `404` so unrelated matters do not announce themselves.

## Where SurrealDB authorization lives

The schema carries `PERMISSIONS NONE`, so everything above this heading describes authorization that lives *outside* the
database. SurrealDB is not neutral that way: every table carries a `PERMISSIONS` clause and the engine evaluates it per
row against the authenticated session. Adopting it (#1093) therefore forced a choice, settled in
[#1145](https://github.com/neon-law-source-code/navigator/issues/1145).

**Decision: authorization stays above the database.** Every Navigator process signs in to Surreal as root, and every
table in `store/src/schema/navigator.surql` is defined with an explicit `PERMISSIONS NONE` clause. The tier and scope
answers stay exactly where the rest of this document puts them: `person.role`, `person_project_role.participation`, and
embedded Rego remain the sole decision point. The two rejected alternatives were mirroring the model into per-table
`PERMISSIONS FOR select` expressions over a non-root session, and splitting by surface (root for trusted server
processes, a scoped session for anything closer to a user).

Three reasons, in the order they decided it:

1. **The conflict check needs an unscoped read, so the engine cannot be the general backstop.** The one traversal this
   store performs today is exactly the query a mirrored policy would have to exempt (see below). A policy layer whose
   first real consumer needs a carve-out is not enforcing much.
2. **Two languages that must agree is the expensive failure.** A mirrored rule that drifts is either over-disclosure —
   a confidentiality breach — or under-disclosure, which in a conflict check means a *missed* conflict. Neither shows up
   as an error; both show up as a rule violation. One model in one language, tested where it is enforced, is the safer
   trade at this size.
3. **There is no non-root consumer yet.** Building the scoped-session credential lane now would ship an authorization
   path nothing exercises, which is how untested authorization gets trusted.

`PERMISSIONS NONE` is written out on every table rather than left to the engine's default, so the schema reads as a
decision and not an omission; `store::schema`'s tests fail if a `DEFINE TABLE` ever lands without a clause. Note what
`NONE` actually buys: it denies every non-root session outright, and root bypasses it. The security property comes from
the connection, so the clause is a fail-closed backstop for a session that does not exist yet, not the live gate.

**What reopens this.** The first surface that hands query power closer to a user — a folder-lane read daemon, an MCP
tool issuing SurrealQL, a debugging console — does not get the root credential. It lands its own scoped non-root session
with the per-table `PERMISSIONS FOR select` expressions that session needs, and settles the credential in each
deployment's `secrets.enc.yaml` at the same time. That is position 3 of #1145, deferred to its first real consumer
rather than declined.

### Whether a conflict check may see across matters

**Yes, and it must.** `store::conflicts` walks `person -> entity_role -> entity -> relationship` from the proposed
client and entity, and the parties it is looking for are by definition on *other* matters — that is what imputed
firm-wide conflict checking under Model Rule 1.10 means. A traversal scoped to the requesting person's participations
would be structurally incapable of finding the undisclosed adversity it exists to find.

`store::conflicts` carries a test that fails if the answer changes: a proposed client with no participation in the
existing matter still raises the finding. The containment is unchanged and lives above the store — the check runs only
on firm-side create paths, and a finding's text reaches a client through nothing.

## Related

- [`docs/oidc.md`](oidc.md) — Authorization Code + PKCE login flow and how the person row is upserted.
- `portal/policy/navigator.rego` — the embedded Rego. `portal::policy` — the `require_policy` middleware that
  evaluates it in process.
- [`docs/glossary.md`](glossary.md) — Person, Project, Disclosure, Participation.
