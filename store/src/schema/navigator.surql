-- Navigator's SurrealDB schema.
--
-- Applied by `store::schema::apply`, which runs this file verbatim and
-- then records `schema_version`. Every statement is idempotent, so a
-- process may apply it on every boot: `IF NOT EXISTS` on tables (which
-- preserves the rows already there) and `OVERWRITE` on fields (so a
-- field definition converges on what this file says).
--
-- SCOPE. The phase-2 conflicts slice, the phase-3 `email_token` slice,
-- the persons slice (#1093, "Schema + migration plan"; ENG-19), the
-- jurisdictions slice, the wave-one flat reference tables —
-- `entity_types`, `glossary_terms`, `git_repositories`,
-- and the wave-two tables: `sent_email`,
-- `visitor_route_count`, the mail chain `address` -> `mailroom` ->
-- `letter`, and the projects cluster — `project`, `person_project_role`,
-- `project_module`, `statutory_deadline`,
-- `git_access_token`, and `testimonial` (ENG-119) — and the wave-four
-- entities cluster: `entity`, the `entity_role` and `relationship`
-- graph edges, and `relationship_log` (ENG-120), plus
-- `person_external_identity` (ENG-85), which is Surreal-native from the
-- start rather than ported — and the
-- wave-five questionnaire pair `question` and `answer`, plus the
-- two-document-lanes pair `asset` and `template` (ENG-121).
-- Postgres still owns the remaining tables and keeps owning them until
-- cutover; a table lands here only when the slice that ports its queries
-- lands.
--
-- THE CONFLICT GRAPH IS THIS SCHEMA. It was not, until wave four.
-- `store::conflicts` used to load `person_entity_roles` and
-- `relationship_edges` out of Postgres and project a name-only copy
-- into a throwaway in-memory database per check, because the rows it
-- traverses lived in the other engine. `conflict_graph.surql` held that
-- copy's definitions, and the two files had to be kept from drifting by
-- hand.
--
-- Wave four made the rows resident, so there is nothing left to project
-- into: `entity_role` and `relationship` below are the graph, and the
-- traversal runs against the deployment's own connection. The second
-- file is deleted rather than maintained — a projection of a table into
-- a copy of itself is a second source of truth that buys nothing once
-- both live in one engine. The traversal is read-only by construction
-- (`LET` + `SELECT`), which `store::conflicts` pins with a test.
--
-- PERMISSIONS. Every table below says `PERMISSIONS NONE` out loud. That
-- is the engine's default, and writing it anyway is the point: #1145
-- settled that Navigator's authorization stays above the database
-- (`persons.role` + `person_project_roles.participation` + embedded Rego policy), so the
-- absence of a per-row rule here has to read as a decision rather than
-- an omission. `store::schema`'s tests fail if a `DEFINE TABLE` lands
-- without a clause. The reasoning, and what would reopen it, live in
-- docs/access-model.md § "Where SurrealDB authorization lives".

-- The applied schema version. `store::schema` reads and writes exactly
-- one record, `schema_version:current`; see `SCHEMA_VERSION`.
DEFINE TABLE IF NOT EXISTS schema_version SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE version ON schema_version TYPE int;
DEFINE FIELD OVERWRITE applied_at ON schema_version TYPE datetime DEFAULT time::now();

-- `projects` cluster — project ids continue to be UUIDs across both engines.
-- `entity_id` is a real `record<entity>` link since the entities slice
-- (ENG-120); the person and project links below are still read back by their
-- command seams, and so is this one, because the engine does not validate a
-- link.
DEFINE TABLE IF NOT EXISTS project SCHEMAFULL PERMISSIONS NONE;
-- A Project code is a route segment and a repository name.
-- `/app/projects/new` is the matter-open form, so a Project whose code is
-- `new` would collide with it; refusing the code in
-- `store::projects::is_valid_code` alone would leave a direct write able to
-- create the colliding row. The slug *shape* stays enforced in Rust, at every
-- write path, rather than being restated here as a second regex to keep in
-- step.
--
-- `READONLY` makes immutability structural rather than a property of which
-- handlers happen to omit `code` from their update commands (see
-- `docs/glossary.md#project`): a Project's route, portal mount, and
-- documents-bucket prefix all key off the exact spelling chosen at
-- matter-open, so a direct write that rewrites it must be refused by the
-- engine itself, not only by the absence of a handler that offers to.
DEFINE FIELD OVERWRITE code ON project TYPE string
    READONLY
    ASSERT $value != 'new';
DEFINE FIELD OVERWRITE name ON project TYPE string;
DEFINE FIELD OVERWRITE status ON project TYPE string;
DEFINE FIELD OVERWRITE entity_id ON project TYPE record<entity>;
DEFINE FIELD OVERWRITE description ON project TYPE option<string>;
DEFINE FIELD OVERWRITE drive_folder_id ON project TYPE option<string>;
-- The full URL of the one source repository this Project's templates and
-- client portal live in. A whole URL rather than a name composed onto one
-- deployment-wide forge host: a Project's source may sit on any forge, in any
-- organization, so GitHub, GitLab, and a self-hosted remote are all just
-- values here. Absent means this Project has no repository recorded, and
-- nothing derives one from `code` — a guessed URL would point at a namespace
-- the Firm may not control. The *shape* is validated in Rust at every write
-- path (`store::projects::is_valid_repository_url`) rather than restated here.
DEFINE FIELD OVERWRITE repository_url ON project TYPE option<string>;
DEFINE FIELD OVERWRITE git_initialized_at ON project TYPE option<string>;
DEFINE FIELD OVERWRITE forge_provisioned_at ON project TYPE option<string>;
DEFINE FIELD OVERWRITE closed_at ON project TYPE option<string>;
DEFINE FIELD OVERWRITE internal_slack_channel_url ON project TYPE option<string>;
DEFINE FIELD OVERWRITE external_slack_channel_url ON project TYPE option<string>;
-- The firm-only Slack Web API channel ID. The bot addresses a channel by this
-- stable ID; the URL above remains an optional human-facing resource link.
DEFINE FIELD OVERWRITE internal_slack_channel_id ON project TYPE option<string>;
-- The matter's two Notion pages: the firm-only working page and the page
-- shared with the client. Two columns rather than one for the same reason the
-- two Slack channels are separate — one side is firm work product and the
-- other is client-visible, so conflating them would make a mistaken paste the
-- only thing standing between the two audiences.
--
-- Navigator stores the address and nothing else. Who may open a Notion page is
-- governed by Notion's own sharing, which Navigator neither reads nor
-- enforces. The link shape is validated in Rust at every write path
-- (`store::projects::is_valid_resource_url`) because each value is rendered as
-- an `href`; that is not restated here as a second rule to keep in step.
DEFINE FIELD OVERWRITE private_notion_page_url ON project TYPE option<string>;
DEFINE FIELD OVERWRITE shared_notion_page_url ON project TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON project TYPE string;
DEFINE FIELD OVERWRITE updated_at ON project TYPE string;
DEFINE INDEX OVERWRITE project_code ON project FIELDS code UNIQUE;

-- `xero_invoice`: local mirror of the one Xero invoice per Project. Xero
-- remains the receivables ledger and the place an invoice is raised; this
-- pointer supplies the portal card and lawyer link without a live provider
-- request. The unique project index is the idempotency and concurrency
-- boundary for the nightly reconcile's journaled write: several rows may
-- carry `NONE` under a Surreal unique index, but a real duplicate Project
-- must still be refused.
DEFINE TABLE IF NOT EXISTS xero_invoice SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON xero_invoice TYPE record<project>;
DEFINE FIELD OVERWRITE xero_invoice_id ON xero_invoice TYPE string;
DEFINE FIELD OVERWRITE reference ON xero_invoice TYPE string;
DEFINE FIELD OVERWRITE status ON xero_invoice TYPE string;
DEFINE FIELD OVERWRITE amount_cents ON xero_invoice TYPE int;
DEFINE FIELD OVERWRITE amount_paid_cents ON xero_invoice TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE currency ON xero_invoice TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON xero_invoice TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON xero_invoice TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE xero_invoice_project ON xero_invoice FIELDS project_id UNIQUE;

DEFINE TABLE IF NOT EXISTS person_project_role SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON person_project_role TYPE record<person>;
DEFINE FIELD OVERWRITE project_id ON person_project_role TYPE record<project>;
DEFINE FIELD OVERWRITE participation ON person_project_role TYPE string;
DEFINE FIELD OVERWRITE is_lawyer_dri ON person_project_role TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE is_client_dri ON person_project_role TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE inserted_at ON person_project_role TYPE string;
DEFINE FIELD OVERWRITE updated_at ON person_project_role TYPE string;
DEFINE INDEX OVERWRITE person_project_role_pair ON person_project_role FIELDS person_id, project_id UNIQUE;

DEFINE TABLE IF NOT EXISTS project_module SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON project_module TYPE record<project>;
DEFINE FIELD OVERWRITE module ON project_module TYPE string
    ASSERT $value IN ['litigation', 'cap_table', 'estate', 'deadlines'];
DEFINE FIELD OVERWRITE enabled_at ON project_module TYPE string;
DEFINE FIELD OVERWRITE enabled_by_person_id ON project_module TYPE option<record<person>>;
DEFINE FIELD OVERWRITE inserted_at ON project_module TYPE string;
DEFINE FIELD OVERWRITE updated_at ON project_module TYPE string;
DEFINE INDEX OVERWRITE project_module_pair ON project_module FIELDS project_id, module UNIQUE;


DEFINE TABLE IF NOT EXISTS statutory_deadline SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON statutory_deadline TYPE record<project>;
DEFINE FIELD OVERWRITE kind ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE trigger_on ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE due_on ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE statute ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE source ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE status ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON statutory_deadline TYPE string;
DEFINE FIELD OVERWRITE updated_at ON statutory_deadline TYPE string;
DEFINE INDEX OVERWRITE statutory_deadline_replay ON statutory_deadline FIELDS project_id, kind, trigger_on, source UNIQUE;

DEFINE TABLE IF NOT EXISTS git_access_token SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON git_access_token TYPE record<person>;
DEFINE FIELD OVERWRITE project_id ON git_access_token TYPE option<record<project>>;
DEFINE FIELD OVERWRITE token_hash ON git_access_token TYPE string;
DEFINE FIELD OVERWRITE scope ON git_access_token TYPE string;
DEFINE FIELD OVERWRITE expires_at ON git_access_token TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON git_access_token TYPE string;
DEFINE FIELD OVERWRITE updated_at ON git_access_token TYPE string;
DEFINE INDEX OVERWRITE git_access_token_hash ON git_access_token FIELDS token_hash UNIQUE;

DEFINE TABLE IF NOT EXISTS testimonial SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON testimonial TYPE record<project>;
DEFINE FIELD OVERWRITE person_id ON testimonial TYPE record<person>;
DEFINE FIELD OVERWRITE quote ON testimonial TYPE string;
DEFINE FIELD OVERWRITE attribution_label ON testimonial TYPE option<string>;
DEFINE FIELD OVERWRITE consented_at ON testimonial TYPE option<string>;
DEFINE FIELD OVERWRITE published_at ON testimonial TYPE option<string>;
DEFINE FIELD OVERWRITE display_order ON testimonial TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE inserted_at ON testimonial TYPE string;
DEFINE FIELD OVERWRITE updated_at ON testimonial TYPE string;
DEFINE INDEX OVERWRITE testimonial_replay ON testimonial FIELDS project_id, person_id, quote UNIQUE;

-- `persons`: the directory of human contacts. The record id is the
-- existing Postgres UUID at import time, so every foreign key ports
-- verbatim.
--
-- `role` is the system-wide authorization tier, and it stays a stored
-- column read *above* the database (#1145) — the ASSERT below is the
-- same gate the Postgres `CHECK (role IN (...))` was, expressed the way
-- `email_token.purpose` expresses its own.
--
-- `email_lower` carries the Postgres `persons_email_lower_key` across.
-- Surreal rejects an expression index outright, so the lowercase form is
-- a stored field with a computed VALUE and the UNIQUE index sits on
-- that. Email is a case-insensitive identifier: `Attorney@Example.com`
-- and `attorney@example.com` are one mailbox, and an IdP may present a
-- casing that differs from the stored row.
--
-- `oidc_subject` is UNIQUE over `option<string>`. Multiple NONEs do not
-- collide on a Surreal unique index, so seeded persons not yet linked to
-- an IdP identity coexist — the same shape the nullable Postgres unique
-- constraint had.
DEFINE TABLE IF NOT EXISTS person SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE name ON person TYPE string;
DEFINE FIELD OVERWRITE given_name ON person TYPE option<string>;
DEFINE FIELD OVERWRITE family_name ON person TYPE option<string>;
DEFINE FIELD OVERWRITE middle_name ON person TYPE option<string>;
DEFINE FIELD OVERWRITE email ON person TYPE string;
DEFINE FIELD OVERWRITE email_lower ON person TYPE string VALUE string::lowercase(email);
DEFINE FIELD OVERWRITE oidc_subject ON person TYPE option<string>;
DEFINE FIELD OVERWRITE role ON person TYPE string DEFAULT 'client'
    ASSERT $value IN ['owner', 'admin', 'lawyer', 'clerk', 'client'];
DEFINE FIELD OVERWRITE title ON person TYPE option<string>;
DEFINE FIELD OVERWRITE phone ON person TYPE option<string>;
DEFINE FIELD OVERWRITE xero_contact_id ON person TYPE option<string>;
DEFINE FIELD OVERWRITE profile_image_url ON person TYPE option<string>;
DEFINE FIELD OVERWRITE linkedin_url ON person TYPE option<string>;
DEFINE FIELD OVERWRITE email_confirmed ON person TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE inserted_at ON person TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON person TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE person_email_lower ON person FIELDS email_lower UNIQUE;
DEFINE INDEX OVERWRITE person_oidc_subject ON person FIELDS oidc_subject UNIQUE;

-- `person_mailbox`: the row that serializes claiming a mailbox, and the
-- reason the UNIQUE `person_email_lower` index above is no longer the
-- whole guard.
--
-- The index does not enforce across concurrent transactions on the
-- embedded engine. Racers creating one mailbox into a free slot land
-- *two* person rows: each transaction writes its own `person` record key
-- and its own index entry, and the optimistic layer conflicts on record
-- keys, not on index entries — so nothing makes the racers collide.
-- Probing for the mailbox first does not help either: a
-- `WHERE email_lower = $v` predicate is a scan, and a scan does not enter
-- the transaction's read set the way a direct record read does.
-- `store/tests/person_mailbox_race.rs` reproduces the fork against the
-- unguarded write and pins the guarded one.
--
-- This table makes them collide. Its **record id is `email_lower`** — the
-- same stored, lowercased spelling the index covers — so every racer
-- claiming one mailbox writes one identical record key and the engine
-- refuses all but one. `CREATE` rather than `UPSERT` is what turns that
-- into a refusal instead of a silent overwrite, and it reports
-- `ErrorDetails::AlreadyExists` — a typed detail, which is what
-- `store::persons::claims_a_mailbox` matches on rather than prose.
--
-- The claim is mutable and releasable because the mailbox is: deleting a
-- person releases their claims, and editing a person's email moves the
-- claim to the new mailbox. A claim that outlived its person would lock
-- that mailbox out of ever being used again.
--
-- `person_email_lower` stays as the backstop for a fork that is not a
-- race — a row written before this table existed, or one whose claim was
-- released without its row going with it.
DEFINE TABLE IF NOT EXISTS person_mailbox SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON person_mailbox TYPE record<person>;

-- `person_external_identity`: the id a third-party system issues for a
-- Person, so Navigator can name them in an API call. An address book and
-- nothing more — no credential, no grant, and no authorization path
-- reads it. `persons.role` is the authorization tier and
-- `person_project_roles` is the scope; an external identity is neither,
-- and `cli/tests/external_identity_is_inert.rs` holds that against the
-- authorization surfaces by name.
--
-- `system` is a stored string gated by an ASSERT, the same shape
-- `person.role` uses. A new system costs one ASSERT entry and one Rust
-- enum variant, not a table.
--
-- `external_id` is the provider's immutable id, never the handle.
-- Handles are renameable, and a renamed handle fails a provisioning call
-- with a 404 at exactly the moment a matter is opening. `handle` is kept
-- for display and is expected to drift.
--
-- As with `credential`, a `record<person>` link is NOT validated against
-- an existing row, so `store::external_identities` checks the Person
-- exists before it writes.
DEFINE TABLE IF NOT EXISTS person_external_identity SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON person_external_identity TYPE record<person>;
DEFINE FIELD OVERWRITE system ON person_external_identity TYPE string
    ASSERT $value IN ['github', 'slack', 'docusign', 'google', 'linear', 'claude', 'chatgpt', 'notion'];
DEFINE FIELD OVERWRITE external_id ON person_external_identity TYPE string;
DEFINE FIELD OVERWRITE handle ON person_external_identity TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON person_external_identity TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON person_external_identity TYPE datetime DEFAULT time::now();
-- One account belongs to one Person...
DEFINE INDEX OVERWRITE person_external_identity_account
    ON person_external_identity FIELDS system, external_id UNIQUE;
-- ...and one Person holds at most one account per system.
DEFINE INDEX OVERWRITE person_external_identity_person_system
    ON person_external_identity FIELDS person_id, system UNIQUE;

-- `jurisdictions`: the reference table of US states (plus DC) and
-- foreign sovereigns an Entity can be organized under — canonical seed
-- data, identical in every deployment, production included. The record
-- id is the UUID `store::jurisdictions` mints, so every cross-engine
-- `jurisdiction_id` still held by a Postgres table addresses it
-- verbatim.
--
-- `code` (`NV`, `CA`, `US`) carries the Postgres unique across as a
-- plain UNIQUE index. Every lookup keyed on it matches exactly — the
-- codes are canonical spellings from `store/seeds/Jurisdiction.yaml`,
-- never user-typed — so no stored lowercase field is needed the way
-- `person.email_lower` is.
--
-- `jurisdiction_type` had no Postgres CHECK, but the domain is closed
-- and load-bearing: the intake `country` question type filters on
-- `jurisdiction_type = 'country'`, so a typo'd value would silently
-- vanish from every picker rather than fail. The ASSERT makes that a
-- write-time error, the same way `person.role` carries its ladder.
DEFINE TABLE IF NOT EXISTS jurisdiction SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE name ON jurisdiction TYPE string;
DEFINE FIELD OVERWRITE code ON jurisdiction TYPE string;
DEFINE FIELD OVERWRITE jurisdiction_type ON jurisdiction TYPE string
    ASSERT $value IN ['state', 'country'];
DEFINE FIELD OVERWRITE inserted_at ON jurisdiction TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON jurisdiction TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE jurisdiction_code ON jurisdiction FIELDS code UNIQUE;

-- `credentials`: a person's licensure in a jurisdiction — the pair that
-- makes an attorney an attorney in a state. Both references are real
-- links now: `person_id` since the persons slice, `jurisdiction_id`
-- since the jurisdictions slice moved the table above.
--
-- A `record<T>` link is NOT validated against an existing row — the
-- engine accepts a link to a person or jurisdiction that was never
-- written, with no constraint violation to catch it. Nothing here
-- enforces referential integrity, so `store::credentials` reads both
-- rows back before it writes.
DEFINE TABLE IF NOT EXISTS credential SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON credential TYPE record<person>;
DEFINE FIELD OVERWRITE jurisdiction_id ON credential TYPE record<jurisdiction>;
DEFINE FIELD OVERWRITE license_number ON credential TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON credential TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON credential TYPE datetime DEFAULT time::now();
-- Carries the Postgres unique `(person_id, jurisdiction_id)` across: the
-- same attorney cannot be double-listed under one jurisdiction.
DEFINE INDEX OVERWRITE credential_person_jurisdiction ON credential
    FIELDS person_id, jurisdiction_id UNIQUE;

-- `entity_types`: the kinds of legal entity an organization can be
-- (LLC, Trust, Corporation, …) — canonical seed data, identical in
-- every deployment. The record id is the UUID `store::entity_types`
-- mints, so the cross-engine `entity_type_id` still held by the
-- Postgres `entities` table addresses it verbatim.
--
-- `name` carries the Postgres unique across as a plain UNIQUE index.
-- The names are canonical seed spellings (`store/seeds/EntityType.yaml`)
-- that every lookup matches exactly — no stored lowercase twin needed
-- the way `person.email_lower` is.
DEFINE TABLE IF NOT EXISTS entity_type SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE name ON entity_type TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON entity_type TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON entity_type TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE entity_type_name ON entity_type FIELDS name UNIQUE;

-- `entities`: a legal organization — an LLC, trust, corporation — or a
-- `Human` entity standing for a solo natural person. A matter is always
-- opened against one, which is why `project.entity_id` above is required.
--
-- Until wave four (ENG-120) this table held only `name`: it was a graph
-- node for the conflicts projection while Postgres owned the real row.
-- The full row lives here now, and `entity_type_id` and `jurisdiction_id`
-- are real `record<>` links rather than the cross-engine UUIDs the
-- Postgres row carried — both reference tables ported ahead of this one,
-- so the ids they already addressed verbatim just became links.
--
-- The engine does not validate a link, so `store::entities` reads both
-- rows back before it writes, exactly as `store::credentials` does.
--
-- `firm_anchor_key` is what replaced a Postgres advisory lock. The firm's
-- own Entity row may not be forked, renamed, or deleted, and that guard
-- was a read (`does this name exist?`) followed by a write —
-- `pg_advisory_xact_lock` made the pair atomic. SurrealDB has no advisory
-- lock and a multi-statement query is not one transaction, so the
-- invariant moves into the schema: `store::entity_commands` stores the
-- lowercased name here when (and only when) the row is an anchor. Multiple
-- NONEs do not collide on a Surreal unique index, so ordinary namesakes —
-- deliberately non-unique, because namesakes are real — never take part.
--
-- The UNIQUE index below refuses a second anchor, but **only when the two
-- writes are not concurrent**; the `firm_anchor` table after it is what
-- serializes racers. Read both before changing either.
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE name ON entity TYPE string;
DEFINE FIELD OVERWRITE entity_type_id ON entity TYPE record<entity_type>;
DEFINE FIELD OVERWRITE jurisdiction_id ON entity TYPE record<jurisdiction>;
DEFINE FIELD OVERWRITE phone ON entity TYPE option<string>;
DEFINE FIELD OVERWRITE url ON entity TYPE option<string>;
DEFINE FIELD OVERWRITE firm_anchor_key ON entity TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON entity TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON entity TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE entity_firm_anchor ON entity FIELDS firm_anchor_key UNIQUE;

-- `firm_anchor`: the row that serializes minting the anchor, and the
-- reason the UNIQUE index above is no longer the whole guard.
--
-- The index does not enforce across concurrent transactions on the
-- embedded engine. Eight racers minting the anchor into a free slot land
-- *two* rows: each transaction writes its own `entity` record key and its
-- own index entry, and the optimistic layer conflicts on record keys, not
-- on index entries — so nothing makes the racers collide.
-- `store/tests/firm_anchor_race.rs` reproduces it against the unguarded
-- write and pins the guarded one.
--
-- This table makes them collide. Its **record id is the anchor key**, so
-- every racer minting the same anchor writes one identical record key and
-- the engine refuses all but one. `CREATE` rather than `UPSERT` is what
-- turns that into a refusal instead of a silent overwrite, and it reports
-- `ErrorDetails::AlreadyExists` — a typed detail, which is what
-- `store::entities::classify_write` matches on rather than prose.
--
-- "Refuses all but one" is true in the steady state and NOT absolute.
-- ENG-312 measured two concurrent `CREATE`s of this same record id both
-- reporting success while this table is still **empty** — about one
-- double-commit per 160,000 eight-way raced creates, on both the embedded
-- and the server engine, with no retry involved. Once any claim row
-- exists, 480,000 raced creates produced no fork. The exposure is
-- therefore the first mint on a fresh database, not ongoing operation.
-- Do not read the paragraph above as a guarantee the engine gives.
--
-- The index stays as the backstop for a fork that is not a race.
DEFINE TABLE IF NOT EXISTS firm_anchor SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE entity_id ON firm_anchor TYPE record<entity>;

-- `person_entity_roles`: the structural ties the conflict check walks at
-- full confidence — a person manages / owns / is a member of an entity.
-- The Postgres `(person_id, entity_id)` pair is the relation's own `in`
-- and `out`, so only `role` remains a field.
--
-- The Postgres table carried no unique constraint, and every writer —
-- `store::seed`, `import::apply` — hand-rolled a find-then-insert to stay
-- idempotent. The index below makes that race-safe rather than merely
-- usually-right: `store::entity_roles::grant` re-reads the winner's row
-- when it loses. A person may hold several roles in one entity, so the
-- tie is the triple, not the pair.
DEFINE TABLE IF NOT EXISTS entity_role TYPE RELATION FROM person TO entity SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE role ON entity_role TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON entity_role TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON entity_role TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE entity_role_tie ON entity_role FIELDS in, out, role UNIQUE;

-- `relationship_edges`: the polymorphic (from_type, from_id, to_type,
-- to_id) quad collapses into native record links on both ends.
-- `confidence_pct` keeps the Postgres CHECK as an ASSERT — the floors
-- (REVIEW_FLOOR_PCT, BLOCK_FLOOR_PCT) stay in Rust, because they decide
-- which paths are worth following and that is a judgment about conflicts
-- rather than about graphs.
--
-- Provenance is load-bearing and travels with the edge: `source_kind`
-- says whether a human asserted it or an LLM parsed it out of free text,
-- and a finding shows that so lawyers judge an edge rather than trust it.
-- `source_id` names the originating row — a `relationship_log` for an
-- LLM-parsed edge, a `disclosure` for a derived one. It stays an
-- unenforced `uuid` rather than a link because it is polymorphic across
-- two tables, one of which (`disclosures`) is still Postgres.
DEFINE TABLE IF NOT EXISTS relationship TYPE RELATION FROM person|entity TO person|entity SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE kind ON relationship TYPE string;
DEFINE FIELD OVERWRITE confidence_pct ON relationship TYPE int ASSERT $value >= 0 AND $value <= 100;
DEFINE FIELD OVERWRITE source_kind ON relationship TYPE string
    ASSERT $value IN ['manual', 'disclosure', 'relationship_log', 'llm'];
DEFINE FIELD OVERWRITE source_id ON relationship TYPE option<uuid>;
DEFINE FIELD OVERWRITE detail ON relationship TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON relationship TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON relationship TYPE datetime DEFAULT time::now();

-- `relationship_logs`: the append-only audit trail of relationship
-- changes — `person joined entity`, `project closed`. One-sided by
-- design: an actor took an action against a subject, which is a
-- different question from the two-sided edges above, and the two are
-- deliberately not merged. The log *feeds* the graph (an LLM may later
-- parse an edge out of `detail`, landing it as a `relationship_log`
-- edge above), but it is not part of it and the traversal never reads
-- it.
--
-- It moves in this wave because its writers do. `store::projects` and
-- `store::project_modules` are already Surreal-resident and were
-- reaching back into Postgres for this one insert, which made a matter
-- open a two-engine write with no transaction over it.
--
-- Append-only — never updated, never deleted — so no unique index and
-- no ASSERT on `action`: the trail records what happened, including
-- actions this code has no closed set for. `subject_type` is a table
-- name and `subject_id` its key; the pair is polymorphic across tables
-- on both sides of the port, so it stays an unenforced `uuid` rather
-- than becoming a link.
DEFINE TABLE IF NOT EXISTS relationship_log SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE actor_person_id ON relationship_log TYPE option<record<person>>;
DEFINE FIELD OVERWRITE subject_type ON relationship_log TYPE string;
DEFINE FIELD OVERWRITE subject_id ON relationship_log TYPE uuid;
DEFINE FIELD OVERWRITE action ON relationship_log TYPE string;
DEFINE FIELD OVERWRITE detail ON relationship_log TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON relationship_log TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON relationship_log TYPE datetime DEFAULT time::now();
-- The lawyer listing reads newest-first; without this it scans.
DEFINE INDEX OVERWRITE relationship_log_inserted_at ON relationship_log FIELDS inserted_at;


-- `git_repositories`: provenance for repositories that carried imported
-- notation content. Rows come from the canonical seed; `navigator
-- site seed` seeds from a local directory and never writes here.
-- `remote_hash` (a SHA-256 of `git remote get-url origin`) carries the
-- Postgres unique across.
DEFINE TABLE IF NOT EXISTS git_repository SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE remote_hash ON git_repository TYPE string;
DEFINE FIELD OVERWRITE last_commit_sha ON git_repository TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON git_repository TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON git_repository TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE git_repository_remote_hash ON git_repository FIELDS remote_hash UNIQUE;

-- `glossary_terms`: the firm vocabulary as reference data (#894) —
-- authored in `docs/glossary.md`, materialized here by
-- `store::glossary::materialize`, keyed by the published-docs anchor
-- `slug`. Canonical seed data, identical in every deployment.
--
-- Deliberately no `project_id` and no link to any matter-scoped table:
-- a contract's defined terms are matter data and must never share this
-- table with firm vocabulary. The shape is pinned by
-- `store/tests/glossary_terms.rs`, which introspects the applied schema.
DEFINE TABLE IF NOT EXISTS glossary_term SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE slug ON glossary_term TYPE string;
DEFINE FIELD OVERWRITE title ON glossary_term TYPE string;
DEFINE FIELD OVERWRITE body ON glossary_term TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON glossary_term TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON glossary_term TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE glossary_term_slug ON glossary_term FIELDS slug UNIQUE;

-- `email_tokens`: single-use, expiring proofs that someone controls an
-- address — the link in a "reset your password" or "confirm your email"
-- message. `store::email_tokens` owns every query against it.
--
-- `person_id` is a `record<person>` link now that the row above exists,
-- so the reference no longer crosses engines. Note the engine does not
-- validate a link, so a token minted for a person that was never written
-- is accepted here and caught in Rust.
--
-- The instants are native `datetime` rather than the RFC 3339 strings
-- Postgres held. The engine parses on write, so an unparseable stored
-- timestamp — which the Postgres lane had to defend against by treating
-- it as expired — cannot occur here.
DEFINE TABLE IF NOT EXISTS email_token SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON email_token TYPE record<person>;
DEFINE FIELD OVERWRITE email ON email_token TYPE string;
DEFINE FIELD OVERWRITE purpose ON email_token TYPE string
    ASSERT $value IN ['password_reset', 'email_confirm'];
DEFINE FIELD OVERWRITE token_hash ON email_token TYPE string;
DEFINE FIELD OVERWRITE expires_at ON email_token TYPE datetime;
DEFINE FIELD OVERWRITE used_at ON email_token TYPE option<datetime>;
DEFINE FIELD OVERWRITE inserted_at ON email_token TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON email_token TYPE datetime DEFAULT time::now();
-- Carries the Postgres `unique_key()` on `token_hash` across. The
-- throttle index pairs it, matching `idx_email_tokens_person_purpose`.
DEFINE INDEX OVERWRITE email_token_hash ON email_token FIELDS token_hash UNIQUE;
DEFINE INDEX OVERWRITE email_token_person_purpose ON email_token FIELDS person_id, purpose;

-- `sent_emails`: one row per outbound message that went through
-- `EmailService`, written by `portal::email`'s `LoggingEmail` decorator
-- and read by `/lawyer/email-log`. Append-only — never updated in place,
-- never deleted — so it carries no unique index and no ASSERTs: the
-- audit trail records what was attempted, including an `outcome` this
-- code has no closed set for (`failed:<reason>` carries the backend's
-- own message).
--
-- `sent_at` is a native `datetime` rather than the RFC 3339 string
-- Postgres held, the same trade `email_token` above makes. It also
-- makes the listing's ORDER BY a real chronological sort instead of a
-- lexicographic one that happened to agree.
--
-- `sg_message_id` is SendGrid's `X-Message-Id`, the join key to the
-- delivery-side Event Webhook stream. `NONE` for failed sends and for
-- the capturing dev backend, neither of which yields an upstream id.
DEFINE TABLE IF NOT EXISTS sent_email SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE recipient ON sent_email TYPE string;
DEFINE FIELD OVERWRITE subject ON sent_email TYPE string;
DEFINE FIELD OVERWRITE sender ON sent_email TYPE string;
DEFINE FIELD OVERWRITE template_slug ON sent_email TYPE option<string>;
DEFINE FIELD OVERWRITE body ON sent_email TYPE string;
DEFINE FIELD OVERWRITE outcome ON sent_email TYPE string;
DEFINE FIELD OVERWRITE sg_message_id ON sent_email TYPE option<string>;
DEFINE FIELD OVERWRITE sent_at ON sent_email TYPE datetime;
DEFINE FIELD OVERWRITE inserted_at ON sent_email TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON sent_email TYPE datetime DEFAULT time::now();
-- Carries `idx_sent_emails_sent_at` across: the email log's hot read is
-- a newest-first paginated listing, which scans without this.
DEFINE INDEX OVERWRITE sent_email_sent_at ON sent_email FIELDS sent_at;

-- `visitor_route_counts`: aggregate public-website visit counters, one
-- row per (day, country, route, source, locale, status class) bucket.
--
-- This table stores bounded counters and nothing else. It deliberately
-- has no field for an IP address, a user agent, a raw path, a raw query
-- string, a session id, a person id, or a full referrer URL — the shape
-- is the privacy guarantee, so adding one of those here is the thing to
-- refuse rather than to review.
--
-- `bucket_date` is a `datetime` pinned to midnight UTC, standing in for
-- the Postgres `date` column, which has no Surreal equivalent. The
-- summary formats it (`%Y-%m-%d`, `%Y-%m`) rather than storing a second
-- derived column.
--
-- The unique index carries the Postgres `ON CONFLICT` target across:
-- it is what makes `record_visit` an upsert rather than an appender, and
-- `store::visitor_analytics` discriminates on its name when a
-- concurrent visit loses the race to create the bucket.
DEFINE TABLE IF NOT EXISTS visitor_route_count SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE bucket_date ON visitor_route_count TYPE datetime;
DEFINE FIELD OVERWRITE country_code ON visitor_route_count TYPE string;
DEFINE FIELD OVERWRITE route_pattern ON visitor_route_count TYPE string;
DEFINE FIELD OVERWRITE source ON visitor_route_count TYPE string;
DEFINE FIELD OVERWRITE locale ON visitor_route_count TYPE string;
DEFINE FIELD OVERWRITE status_class ON visitor_route_count TYPE string;
DEFINE FIELD OVERWRITE visits ON visitor_route_count TYPE int ASSERT $value >= 0;
DEFINE FIELD OVERWRITE inserted_at ON visitor_route_count TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON visitor_route_count TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE visitor_route_count_bucket ON visitor_route_count
    FIELDS bucket_date, country_code, route_pattern, source, locale, status_class UNIQUE;

-- `addresses`: a postal address, attached to a person, to an entity, or
-- to neither.
--
-- Both `person_id` and `entity_id` are real `record<>` links: the
-- persons slice made the first one, and the entities slice (ENG-120)
-- made the second, retiring the last cross-engine `uuid` this table
-- carried.
--
-- There is deliberately no XOR assert. The Postgres comment described
-- the person/entity exclusivity as application-enforced "to stay
-- portable", but the seed's mailroom placeholder address sets *both* to
-- NONE, so an `ASSERT` spelling the XOR out would reject a row the
-- canonical seed writes on every boot.
DEFINE TABLE IF NOT EXISTS address SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE person_id ON address TYPE option<record<person>>;
DEFINE FIELD OVERWRITE entity_id ON address TYPE option<record<entity>>;
DEFINE FIELD OVERWRITE line1 ON address TYPE string;
DEFINE FIELD OVERWRITE line2 ON address TYPE option<string>;
DEFINE FIELD OVERWRITE city ON address TYPE string;
DEFINE FIELD OVERWRITE region ON address TYPE string;
DEFINE FIELD OVERWRITE postal_code ON address TYPE string;
DEFINE FIELD OVERWRITE country ON address TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON address TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON address TYPE datetime DEFAULT time::now();

-- `mailrooms`: a physical mail-receiving destination.
--
-- `address_id` is a real `record<address>` link, because `addresses`
-- landed in the commit before this one — the whole reason the trio moved
-- together. The engine still does not validate it, so
-- `store::mailrooms::create` reads the address back before writing;
-- `fk_mailrooms_address` used to do that job, and the read-back is what
-- replaced it, not the `record<T>` type.
DEFINE TABLE IF NOT EXISTS mailroom SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE name ON mailroom TYPE string;
DEFINE FIELD OVERWRITE address_id ON mailroom TYPE record<address>;
DEFINE FIELD OVERWRITE inserted_at ON mailroom TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON mailroom TYPE datetime DEFAULT time::now();
-- Carries the Postgres unique on `name` across.
DEFINE INDEX OVERWRITE mailroom_name ON mailroom FIELDS name UNIQUE;

-- `letters`: one physical piece of mail, incoming or outgoing.
--
-- `mailroom_id` is a real `record<mailroom>` link, the last hop of the
-- `letter -> mailroom -> address` chain this wave moved as a unit. The
-- engine does not validate it, so `store::letters::record` reads the
-- mailroom back before writing — the replacement for the dropped
-- `fk_letters_mailroom`.
--
-- `direction` carries the closed set the Postgres column only held by
-- convention. The lawyer listing and the inbound router both write
-- `incoming`; an outgoing letter is a real value, and a typo'd third
-- one would silently vanish from both surfaces rather than fail.
DEFINE TABLE IF NOT EXISTS letter SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE mailroom_id ON letter TYPE record<mailroom>;
DEFINE FIELD OVERWRITE direction ON letter TYPE string
    ASSERT $value IN ['incoming', 'outgoing'];
DEFINE FIELD OVERWRITE sender ON letter TYPE string;
DEFINE FIELD OVERWRITE recipient ON letter TYPE string;
DEFINE FIELD OVERWRITE summary ON letter TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON letter TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON letter TYPE datetime DEFAULT time::now();

-- `questions`: one prompt presented to a respondent during template
-- traversal. Seeded from template frontmatter by `cli import`, so the
-- lawyer surface is a transparency listing only — no add, edit, or delete.
--
-- `code` carries the Postgres unique across. The codes are canonical
-- identifiers written by a template author (`client_name`, `trustee_name`)
-- and matched exactly by every lookup, so no stored lowercase twin is
-- needed the way `person.email_lower` is.
--
-- `audience` carries a closed set the Postgres column only held by
-- convention (NOT NULL DEFAULT 'both', no CHECK). It is load-bearing:
-- `store::questions::is_client_facing` reads it to decide whether the
-- client's magic-link surface shows the question at all, so a typo'd value
-- would silently hide a question from the client rather than fail. The
-- ASSERT makes that a write-time error, the same way `letter.direction`
-- carries its set.
DEFINE TABLE IF NOT EXISTS question SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE code ON question TYPE string;
DEFINE FIELD OVERWRITE prompt ON question TYPE string;
DEFINE FIELD OVERWRITE answer_type ON question TYPE string;
DEFINE FIELD OVERWRITE audience ON question TYPE string DEFAULT 'both'
    ASSERT $value IN ['lawyer', 'client', 'both'];
DEFINE FIELD OVERWRITE inserted_at ON question TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON question TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE question_code ON question FIELDS code UNIQUE;

-- `answers`: one respondent's answer to one question.
--
-- Append-only. Re-asks and corrections are new rows, never updates, and
-- latest-per-`(notation_id, state_name)` wins on read — so there is no
-- unique index, and `store::answers` exports no update and no delete. That
-- is the `sent_email` precedent above: the guarantee is carried by the
-- command seam's shape rather than by the engine.
--
-- `person_id` is the **respondent** (whose answer it is);
-- `authored_by_person_id` is who actually typed it — a lawyer filling it in
-- on the client's behalf, or the client through the magic link — so one
-- notation can interleave both authorships and the data lake can tell them
-- apart. `source` is the same dimension as a low-cardinality string.
--
-- `notation_id` stays an unenforced `uuid` rather than a `record<notation>`
-- link: `notations` is still Postgres-resident and moves in a later slice
-- of this wave (ENG-121). It becomes a real link then. `relationship.
-- source_id` above is the same shape for the same reason.
--
-- `value` is the ported JSONB column, and its type is the whole decision.
-- It carries three shapes — a primitive envelope `{"value": …}`, a mirrored
-- record row, and an aggregate array — and on a SCHEMAFULL table only
-- `TYPE any` accepts all three. Verified against the pinned surrealdb
-- 3.2.4, because each near miss fails differently and none of them fail at
-- DEFINE time:
--
--   * `TYPE object` rejects every undefined nested key ("Found field
--     '`value`.value', but no such field exists").
--   * `TYPE object FLEXIBLE` takes arbitrary nesting but rejects the
--     top-level array outright ("Expected `object` but found `[…]`").
--   * `TYPE object | array FLEXIBLE` is worse, not better: FLEXIBLE does
--     not propagate through a union, so nested keys fail again.
--   * `FLEXIBLE TYPE object` does not parse at all — FLEXIBLE must be
--     written AFTER TYPE in 3.2.x.
--
-- `TYPE any` also means the Postgres NOT NULL cannot be carried here: an
-- `ASSERT $value != NONE` does not fire on a NONE-valued `any` field. The
-- not-null guarantee is the command seam's instead — `store::answers`
-- always sets `value`, and nothing else writes this table.
DEFINE TABLE IF NOT EXISTS answer SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE question_id ON answer TYPE record<question>;
DEFINE FIELD OVERWRITE person_id ON answer TYPE record<person>;
DEFINE FIELD OVERWRITE notation_id ON answer TYPE option<uuid>;
DEFINE FIELD OVERWRITE state_name ON answer TYPE option<string>;
DEFINE FIELD OVERWRITE value ON answer TYPE any;
DEFINE FIELD OVERWRITE source ON answer TYPE string DEFAULT 'lawyer'
    ASSERT $value IN ['lawyer', 'client', 'extracted'];
DEFINE FIELD OVERWRITE authored_by_person_id ON answer TYPE option<record<person>>;
DEFINE FIELD OVERWRITE inserted_at ON answer TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON answer TYPE datetime DEFAULT time::now();
-- The hot read is "this notation's answers, in the order they were given",
-- and the walker's per-question read narrows that by state and respondent.
-- Carries `idx_answers_notation_id` and its companions across.
DEFINE INDEX OVERWRITE answer_notation ON answer FIELDS notation_id;
DEFINE INDEX OVERWRITE answer_question_person ON answer FIELDS question_id, person_id;

-- `assets`: one canonical store for static bytes. The bytes themselves
-- live in object storage (`cloud::StorageService`); this row holds the
-- pointer plus, for a matter document, its metadata. Two shapes share the
-- table — a **document asset**, whose `project_id`/`filename`/`kind`/
-- `source`/`received_at`/`description` are populated, and a **bare content
-- asset** (a template body, a raw `.eml`), where they are NONE.
--
-- `storage_key` is content-addressed (`blobs/<sha>`) but deliberately not
-- unique: identical bytes are stored once and may be referenced by several
-- rows. `sha256_hex` is the dedup key, and its index is non-unique for the
-- same reason.
--
-- `project_id` is a real `record<project>` link — `project` has been
-- Surreal-resident since wave two. The engine does not validate a link, so
-- the ingest seam is what checks the matter exists, as everywhere else.
--
-- `visibility` carries the Postgres `assets_visibility_check` CHECK across
-- as an ASSERT. It is the client-portal gate: a typo'd value would drop a
-- document out of the client's "Your documents" silently rather than fail.
--
-- `metadata` is the ported JSONB column and takes `TYPE any` for the reason
-- `answer.value` above does — on a SCHEMAFULL table it is the only type
-- that accepts arbitrary nesting, and FLEXIBLE must be written after TYPE.
-- `store/src/assets.rs` pins the round trip with a test.
--
-- INSERTION ORDER DECIDES CURRENT. `slug` makes a row one revision of a
-- living document, and `(project_id, slug)` is deliberately non-unique.
-- Which revision is operative is `id` descending — ids are UUIDv7, so that
-- is insertion order — never `published_at`, which is back-datable to a
-- court's file stamp and would otherwise let a filing reorder a chain. The
-- index below serves that read; it is the ordering `store::assets::current`
-- and `revisions` depend on.
DEFINE TABLE IF NOT EXISTS asset SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE storage_key ON asset TYPE string;
DEFINE FIELD OVERWRITE secondary_storage_key ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE content_type ON asset TYPE string;
DEFINE FIELD OVERWRITE byte_size ON asset TYPE int;
DEFINE FIELD OVERWRITE sha256_hex ON asset TYPE string;
DEFINE FIELD OVERWRITE project_id ON asset TYPE option<record<project>>;
DEFINE FIELD OVERWRITE filename ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE kind ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE source ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE received_at ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE description ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE visibility ON asset TYPE string DEFAULT 'internal'
    ASSERT $value IN ['internal', 'client'];
DEFINE FIELD OVERWRITE slug ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE published_at ON asset TYPE option<string>;
DEFINE FIELD OVERWRITE metadata ON asset TYPE any;
DEFINE FIELD OVERWRITE inserted_at ON asset TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON asset TYPE datetime DEFAULT time::now();
-- The dedup probe (`sha256_hex`, narrowed by project for a document
-- ingest) and the revision chain read (`project_id, slug` newest-first).
DEFINE INDEX OVERWRITE asset_sha256 ON asset FIELDS sha256_hex;
DEFINE INDEX OVERWRITE asset_project_slug ON asset FIELDS project_id, slug;

-- `templates`: the questionnaire + markdown body that drives a notation,
-- identified by a stable `code`.
--
-- `asset_id` is a real `record<asset>` link, which is the whole reason
-- these two tables move in one slice: `store::templates::save_version`
-- folds `asset_id` into the tuple that decides whether a version changed,
-- so split across engines the lane would hold its version identity in one
-- and the bytes it names in the other. `fk_templates_asset` is already
-- dropped (`m20260911_drop_notation_group_foreign_keys`); what replaces it
-- is `store::templates::body`'s read-back of the asset before it fetches
-- from storage, not the link's type — the engine does not validate a link.
--
-- ROWS ARE IMMUTABLE BY POLICY. An edit appends a new row and flips
-- `is_current`; it never rewrites a spec a Notation pinned through
-- `notation.template_id`.
--
-- `current_key` is how the two Postgres *partial* unique indexes come
-- across — `UNIQUE (code) WHERE project_id IS NULL AND is_current` and
-- `UNIQUE (project_id, code) WHERE is_current`. Surreal has no partial
-- index, so `store::templates` stores a key only while a row is current
-- and clears it when the row retires. Multiple NONEs do not collide on a
-- Surreal unique index, so retired versions never take part, and the
-- shared (`project_id` NONE) and project-scoped lines get distinct keys.
-- This is `entity.firm_anchor_key`'s shape, for the same reason: the
-- invariant is a read followed by a write, and a multi-statement Surreal
-- query is not one transaction, so the index has to refuse the second
-- current row rather than the code serializing the racers.
DEFINE TABLE IF NOT EXISTS template SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE code ON template TYPE string;
DEFINE FIELD OVERWRITE title ON template TYPE string;
DEFINE FIELD OVERWRITE respondent_type ON template TYPE string;
DEFINE FIELD OVERWRITE project_id ON template TYPE option<record<project>>;
DEFINE FIELD OVERWRITE is_current ON template TYPE bool DEFAULT true;
DEFINE FIELD OVERWRITE asset_id ON template TYPE option<record<asset>>;
DEFINE FIELD OVERWRITE form_code ON template TYPE option<string>;
DEFINE FIELD OVERWRITE kind ON template TYPE option<string>;
DEFINE FIELD OVERWRITE source_commit_sha ON template TYPE option<string>;
DEFINE FIELD OVERWRITE current_key ON template TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON template TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON template TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE template_current_key ON template FIELDS current_key UNIQUE;
-- The resolve path reads by code and narrows by scope.
DEFINE INDEX OVERWRITE template_code ON template FIELDS code;

-- ---------------------------------------------------------------------
-- The notation group (wave five, ENG-121)
--
-- Fifteen tables that hang together off `notation`: the core, its
-- satellite ring, the citation apparatus, and the two JSONB-body tables.
-- `m20260911_drop_notation_group_foreign_keys` already removed the
-- Postgres foreign keys across the whole group, so nothing below is
-- replacing a live constraint — the links here are how the reference is
-- *typed*, and the read-back at the command seam is what actually
-- validates it. The engine does not validate a `record<…>` link.
--
-- A reference is a `record<…>` only when the referenced table also lives
-- in Surreal. `document_comment.communication_id` stays a bare `uuid`
-- because `communications` is the comms cluster and moves in a later
-- wave; typing it as a link now would name a table that does not exist.
--
-- JSONB columns land as `TYPE any`, the shape `asset.metadata` already
-- uses. `any` admits NONE, so a nullable JSON column needs no `option<>`
-- wrapper — and must not get one: `FLEXIBLE` may only follow a concrete
-- `TYPE`, and does not survive inside a union like `option<any>`.
--
-- `inserted_at` / `updated_at` are `datetime`, as everywhere else in this
-- file. Domain timestamps that the application passes through as opaque
-- RFC 3339 text (`recorded_at`, `submitted_at`,
-- `signed_at`, `notarized_at`, `issued_on`, `checked_on`) stay `string`,
-- so porting these tables does not also re-type a public seam.
--
-- Every one of the fifteen carried `crate::uuid_active_model_behavior!`,
-- the SeaORM `before_save` hook that stamped `id`, `inserted_at`, and
-- `updated_at`. A hook cannot reach Surreal (the wave-two rule), so each
-- ported write stamps `Uuid::now_v7()` and the timestamps itself. The
-- `DEFAULT time::now()` below is the backstop for a row written without
-- them, not the mechanism.

-- `notations`: one filled-in template, owned by a person (and optionally
-- an entity) inside exactly one Project, with a workflow `state`.
-- `project_id` is the glossary's load-bearing matter-audit-trail rule.
DEFINE TABLE IF NOT EXISTS notation SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE template_id ON notation TYPE record<template>;
DEFINE FIELD OVERWRITE person_id ON notation TYPE record<person>;
DEFINE FIELD OVERWRITE entity_id ON notation TYPE option<record<entity>>;
DEFINE FIELD OVERWRITE project_id ON notation TYPE record<project>;
DEFINE FIELD OVERWRITE state ON notation TYPE string;
DEFINE FIELD OVERWRITE delivery ON notation TYPE string DEFAULT 'embedded';
-- The frozen questionnaire the Notation was opened against, so a later
-- template edit cannot re-route a Notation already in flight.
DEFINE FIELD OVERWRITE questionnaire_snapshot ON notation TYPE any;
DEFINE FIELD OVERWRITE git_commit_sha ON notation TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON notation TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON notation TYPE datetime DEFAULT time::now();
-- The matter listing reads by project; the portal reads a client's own
-- notations by person; the template lane reads back by pinned version.
DEFINE INDEX OVERWRITE notation_project ON notation FIELDS project_id;
DEFINE INDEX OVERWRITE notation_person ON notation FIELDS person_id;
DEFINE INDEX OVERWRITE notation_template ON notation FIELDS template_id;

-- `notation_clauses`: one custom paragraph spliced into a single
-- notation's assembled document at `{{custom_clauses}}`, in `position`
-- order, without forking the shared template.
DEFINE TABLE IF NOT EXISTS notation_clause SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON notation_clause TYPE record<notation>;
DEFINE FIELD OVERWRITE position ON notation_clause TYPE int;
DEFINE FIELD OVERWRITE body_markdown ON notation_clause TYPE string;
DEFINE FIELD OVERWRITE authored_by_person_id ON notation_clause TYPE option<record<person>>;
DEFINE FIELD OVERWRITE inserted_at ON notation_clause TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON notation_clause TYPE datetime DEFAULT time::now();
-- The only read is "this notation's clauses, in position order".
DEFINE INDEX OVERWRITE notation_clause_notation ON notation_clause FIELDS notation_id, position;

-- `notation_events`: the append-only journal of every state-machine
-- transition for every Notation. One row per transition.
--
-- APPEND-ONLY, BY CONSTRUCTION AT THE COMMAND SEAM. Postgres enforced
-- this with the `prevent_notation_events_mutation` trigger
-- (`m20260802_harden_notation_events_journal`), which raised on UPDATE
-- and DELETE. Surreal has no trigger in our usage, so the guarantee moves
-- to `store::notation_events`: it exports an append and reads, and no
-- update or delete function at all, pinned by a covering test. This is
-- the `sent_email` precedent — documented and structurally enforced by
-- what the module does not export, rather than by the engine.
--
-- `payload` is opaque JSON *text*, not a native object: a questionnaire
-- event can carry client answer content, so it is written and read as an
-- opaque blob and must never be logged or traced.
DEFINE TABLE IF NOT EXISTS notation_event SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON notation_event TYPE record<notation>;
DEFINE FIELD OVERWRITE acting_person_id ON notation_event TYPE record<person>;
DEFINE FIELD OVERWRITE template_version_id ON notation_event TYPE record<template>;
DEFINE FIELD OVERWRITE machine_kind ON notation_event TYPE string;
DEFINE FIELD OVERWRITE from_state ON notation_event TYPE string;
DEFINE FIELD OVERWRITE to_state ON notation_event TYPE string;
DEFINE FIELD OVERWRITE condition ON notation_event TYPE string;
DEFINE FIELD OVERWRITE payload ON notation_event TYPE option<string>;
DEFINE FIELD OVERWRITE recorded_at ON notation_event TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON notation_event TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON notation_event TYPE datetime DEFAULT time::now();
-- "Current state" is the newest row for a `(notation_id, machine_kind)`
-- pair — the one read this table exists to serve.
DEFINE INDEX OVERWRITE notation_event_machine ON notation_event FIELDS notation_id, machine_kind;

-- `review_documents`: one attorney-reviewed draft a client reads and
-- comments on before signing. A matter can have several — an estate plan
-- is a will, a trust, and two directives.
DEFINE TABLE IF NOT EXISTS review_document SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON review_document TYPE record<notation>;
DEFINE FIELD OVERWRITE kind ON review_document TYPE string;
DEFINE FIELD OVERWRITE title ON review_document TYPE string;
DEFINE FIELD OVERWRITE body_html ON review_document TYPE string;
DEFINE FIELD OVERWRITE status ON review_document TYPE string DEFAULT 'draft';
DEFINE FIELD OVERWRITE inserted_at ON review_document TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON review_document TYPE datetime DEFAULT time::now();
-- One row per instrument per matter. This carries
-- `idx_review_documents_unique_notation_kind` across, and it is what
-- `store::review_documents::upsert_draft` writes through — the upsert is
-- a read then a write, and a multi-statement Surreal query is not one
-- transaction, so the index is what refuses the second row under a race.
DEFINE INDEX OVERWRITE review_document_notation_kind ON review_document FIELDS notation_id, kind UNIQUE;

-- `document_comments`: one comment a reader anchored to a character range
-- within a review document. The review surface is read-only; a comment is
-- the only thing the client writes there.
DEFINE TABLE IF NOT EXISTS document_comment SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE review_document_id ON document_comment TYPE record<review_document>;
DEFINE FIELD OVERWRITE person_id ON document_comment TYPE record<person>;
DEFINE FIELD OVERWRITE anchor_start ON document_comment TYPE int;
DEFINE FIELD OVERWRITE anchor_end ON document_comment TYPE int;
DEFINE FIELD OVERWRITE quoted_text ON document_comment TYPE string;
DEFINE FIELD OVERWRITE body ON document_comment TYPE string;
DEFINE FIELD OVERWRITE resolved ON document_comment TYPE bool DEFAULT false;
-- Bare `uuid`, not a link: `communications` is the comms cluster and has
-- not moved yet. It becomes `option<record<communication>>` in that wave.
DEFINE FIELD OVERWRITE communication_id ON document_comment TYPE option<uuid>;
DEFINE FIELD OVERWRITE inserted_at ON document_comment TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON document_comment TYPE datetime DEFAULT time::now();
-- The sidebar reads one draft's thread.
DEFINE INDEX OVERWRITE document_comment_review_document ON document_comment FIELDS review_document_id;

-- `filings`: one durable record per outbound compliance submission —
-- mail to a party, or a filing with a government office. Written inside a
-- workflow step's `ctx.run`, so the row is replay-idempotent proof of
-- what was filed.
DEFINE TABLE IF NOT EXISTS filing SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON filing TYPE record<notation>;
DEFINE FIELD OVERWRITE kind ON filing TYPE string;
DEFINE FIELD OVERWRITE office ON filing TYPE string;
DEFINE FIELD OVERWRITE reference ON filing TYPE option<string>;
DEFINE FIELD OVERWRITE summary ON filing TYPE string;
DEFINE FIELD OVERWRITE submitted_at ON filing TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON filing TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON filing TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE filing_notation ON filing FIELDS notation_id;

-- `attestations`: one durable local record per on-chain attorney
-- attestation. The Solana transaction is a mirror recorded only when a
-- real chain backend is configured; the row exists regardless, with
-- `status` distinguishing `pending` from `recorded`.
DEFINE TABLE IF NOT EXISTS attestation SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON attestation TYPE record<notation>;
DEFINE FIELD OVERWRITE chain ON attestation TYPE string;
DEFINE FIELD OVERWRITE sha256 ON attestation TYPE string;
DEFINE FIELD OVERWRITE status ON attestation TYPE string;
DEFINE FIELD OVERWRITE pda ON attestation TYPE option<string>;
DEFINE FIELD OVERWRITE tx_signature ON attestation TYPE option<string>;
DEFINE FIELD OVERWRITE firm_wallet ON attestation TYPE option<string>;
DEFINE FIELD OVERWRITE client_wallet ON attestation TYPE option<string>;
DEFINE FIELD OVERWRITE recorded_at ON attestation TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON attestation TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON attestation TYPE datetime DEFAULT time::now();
-- The upsert helper reads back by matter before writing.
DEFINE INDEX OVERWRITE attestation_notation ON attestation FIELDS notation_id;

-- `signatures`: one signature request/execution on a Notation's document.
-- `provider` was a SeaORM `DeriveActiveEnum` over a TEXT column; on this
-- engine it is the stored string, and the closed set is enforced by
-- `store::signatures` rather than by a column type.
DEFINE TABLE IF NOT EXISTS signature SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON signature TYPE record<notation>;
DEFINE FIELD OVERWRITE signer_person_id ON signature TYPE option<record<person>>;
DEFINE FIELD OVERWRITE field ON signature TYPE option<string>;
DEFINE FIELD OVERWRITE provider ON signature TYPE string;
DEFINE FIELD OVERWRITE provider_id ON signature TYPE string;
DEFINE FIELD OVERWRITE signed_at ON signature TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON signature TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON signature TYPE datetime DEFAULT time::now();
-- The inbound completion webhook's only correlation key back to a matter.
DEFINE INDEX OVERWRITE signature_provider_request ON signature FIELDS provider, provider_id UNIQUE;
DEFINE INDEX OVERWRITE signature_notation ON signature FIELDS notation_id;

-- `notarizations`: the notary counterpart to `signature`, correlated back
-- from the provider by the same `(provider, provider_id)` pair.
DEFINE TABLE IF NOT EXISTS notarization SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON notarization TYPE record<notation>;
DEFINE FIELD OVERWRITE notary_person_id ON notarization TYPE option<record<person>>;
DEFINE FIELD OVERWRITE asset_id ON notarization TYPE option<record<asset>>;
DEFINE FIELD OVERWRITE provider ON notarization TYPE string;
DEFINE FIELD OVERWRITE provider_id ON notarization TYPE string;
DEFINE FIELD OVERWRITE notarized_at ON notarization TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON notarization TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON notarization TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE notarization_provider_request ON notarization FIELDS provider, provider_id UNIQUE;
DEFINE INDEX OVERWRITE notarization_notation ON notarization FIELDS notation_id;

-- `authorities`: a case, statute, regulation, administrative proceeding,
-- or secondary source, as GLOBAL reference data. No `project_id` on
-- purpose — the same authorities recur across matters, and a matter's
-- *use* of one is the separate scoped `authority_use` record.
DEFINE TABLE IF NOT EXISTS authority SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE class ON authority TYPE string;
DEFINE FIELD OVERWRITE citation ON authority TYPE string;
DEFINE FIELD OVERWRITE short_cite ON authority TYPE option<string>;
DEFINE FIELD OVERWRITE title ON authority TYPE string;
DEFINE FIELD OVERWRITE publisher ON authority TYPE option<string>;
DEFINE FIELD OVERWRITE issued_on ON authority TYPE option<string>;
DEFINE FIELD OVERWRITE canonical_url ON authority TYPE option<string>;
DEFINE FIELD OVERWRITE checked_on ON authority TYPE option<string>;
DEFINE FIELD OVERWRITE archived_asset_id ON authority TYPE option<record<asset>>;
DEFINE FIELD OVERWRITE inserted_at ON authority TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON authority TYPE datetime DEFAULT time::now();
-- The full citation is unique across the firm, and the find-or-create on
-- that natural key is what keeps two matters from minting rival rows for
-- the same case under a race.
DEFINE INDEX OVERWRITE authority_citation ON authority FIELDS citation UNIQUE;

-- `authority_uses`: a matter's use of a global authority — the scoped
-- half of the global/scoped split. The opponent's use sits alongside
-- ours, because the same case reads differently depending on which side
-- cites it; a column on `authority` would leak one matter's litigation
-- posture into another matter's view.
DEFINE TABLE IF NOT EXISTS authority_use SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON authority_use TYPE record<project>;
DEFINE FIELD OVERWRITE authority_id ON authority_use TYPE record<authority>;
DEFINE FIELD OVERWRITE position ON authority_use TYPE string;
DEFINE FIELD OVERWRITE disposition ON authority_use TYPE string;
DEFINE FIELD OVERWRITE role ON authority_use TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON authority_use TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON authority_use TYPE datetime DEFAULT time::now();
-- The matter's authority table reads by project and narrows by authority.
DEFINE INDEX OVERWRITE authority_use_project ON authority_use FIELDS project_id, authority_id;

-- `citations`: the Locator — one assertion citing an authority at a
-- specific place, quoting specific text, for a stated reason.
--
-- Matter scoping flows THROUGH `authority_use_id` and is deliberately not
-- denormalized onto this row, so a citation cannot name a different
-- matter than the use it belongs to. Keep that path intact: a reader
-- resolves the use to reach the project, never a `project_id` here.
DEFINE TABLE IF NOT EXISTS citation SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE authority_use_id ON citation TYPE record<authority_use>;
DEFINE FIELD OVERWRITE quote ON citation TYPE string;
DEFINE FIELD OVERWRITE why ON citation TYPE string;
-- Both pins are JSON: a paragraph, a page, or a normalised rect.
DEFINE FIELD OVERWRITE source_pin ON citation TYPE any;
DEFINE FIELD OVERWRITE draft_pin ON citation TYPE any;
DEFINE FIELD OVERWRITE inserted_at ON citation TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON citation TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE citation_authority_use ON citation FIELDS authority_use_id;

-- `verifications`: evidence that a licensed human checked a citation
-- before it was filed. A domain record, not telemetry — it is diligence,
-- it may need to be produced, and it carries the quote and the citation,
-- which are document content. Matter scoping flows through the citation,
-- so this row carries no `project_id` either.
DEFINE TABLE IF NOT EXISTS verification SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE citation_id ON verification TYPE record<citation>;
DEFINE FIELD OVERWRITE revision_sha ON verification TYPE string;
DEFINE FIELD OVERWRITE status_citation ON verification TYPE string;
DEFINE FIELD OVERWRITE status_quote ON verification TYPE string;
DEFINE FIELD OVERWRITE status_proposition ON verification TYPE string;
DEFINE FIELD OVERWRITE verifier_person_id ON verification TYPE record<person>;
DEFINE FIELD OVERWRITE inserted_at ON verification TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON verification TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE verification_citation ON verification FIELDS citation_id;

-- `playbooks`: a client Entity's stored contract-negotiation positions,
-- scoped to the company so one playbook serves every matter for that
-- client. `positions` is a JSON array edited as a whole.
DEFINE TABLE IF NOT EXISTS playbook SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE entity_id ON playbook TYPE record<entity>;
DEFINE FIELD OVERWRITE name ON playbook TYPE string;
DEFINE FIELD OVERWRITE positions ON playbook TYPE any;
DEFINE FIELD OVERWRITE active ON playbook TYPE bool DEFAULT true;
DEFINE FIELD OVERWRITE inserted_at ON playbook TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON playbook TYPE datetime DEFAULT time::now();
-- The label is unique per Entity, and the find-or-create rides that key.
DEFINE INDEX OVERWRITE playbook_entity_name ON playbook FIELDS entity_id, name UNIQUE;

-- `contract_reviews`: the per-notation work-product satellite for an
-- inbound contract review — the review-*in* mirror of `review_document`.
-- `findings` is the deviation report the analysis step produces and the
-- reviewing attorney edits before approving. Per-finding attorney
-- attribution is the matter's audit trail and lives in `notation_event`;
-- this row is the editable working copy.
DEFINE TABLE IF NOT EXISTS contract_review SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE notation_id ON contract_review TYPE record<notation>;
DEFINE FIELD OVERWRITE playbook_id ON contract_review TYPE record<playbook>;
DEFINE FIELD OVERWRITE asset_id ON contract_review TYPE option<record<asset>>;
DEFINE FIELD OVERWRITE status ON contract_review TYPE string DEFAULT 'pending';
DEFINE FIELD OVERWRITE risk_summary ON contract_review TYPE option<string>;
DEFINE FIELD OVERWRITE findings ON contract_review TYPE any;
DEFINE FIELD OVERWRITE inserted_at ON contract_review TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON contract_review TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE contract_review_notation ON contract_review FIELDS notation_id;
DEFINE INDEX OVERWRITE contract_review_playbook ON contract_review FIELDS playbook_id;

-- `expunge_records`: the audit trail of governed expunges. A governed
-- expunge rewrites a matter repo's history to remove privileged, sealed,
-- or lawfully-deleted material; this row records the expunge *itself* —
-- who authorized it, when, the category, and the before/after head oids —
-- and never the content removed, so the redaction is auditable without
-- re-exposing it. See `docs/project-repositories.md` §9.
DEFINE TABLE IF NOT EXISTS expunge_record SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON expunge_record TYPE record<project>;
DEFINE FIELD OVERWRITE path ON expunge_record TYPE string;
-- The Postgres table carried no CHECK; the category constants lived only
-- in Rust. The ASSERT makes an unrecognized category a write-time error
-- rather than a row nothing can classify later.
DEFINE FIELD OVERWRITE category ON expunge_record TYPE string
    ASSERT $value IN ['privilege', 'sealing', 'client_request'];
DEFINE FIELD OVERWRITE authorized_by_person_id ON expunge_record TYPE record<person>;
DEFINE FIELD OVERWRITE head_before ON expunge_record TYPE option<string>;
DEFINE FIELD OVERWRITE head_after ON expunge_record TYPE option<string>;
DEFINE FIELD OVERWRITE note ON expunge_record TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON expunge_record TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON expunge_record TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE expunge_record_project ON expunge_record FIELDS project_id;

-- `expunge_requests`: a client's ask to delete one of their matter
-- documents. The governed-expunge primitive is admin-only, so a client can
-- only *ask*: this row is `pending` until a lawyer/admin authorizes it
-- (running the expunge, with `expunge_record_id` linking the audit row) or
-- denies it. An executed request is always category `client_request`.
DEFINE TABLE IF NOT EXISTS expunge_request SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON expunge_request TYPE record<project>;
DEFINE FIELD OVERWRITE asset_id ON expunge_request TYPE record<asset>;
DEFINE FIELD OVERWRITE requested_by_person_id ON expunge_request TYPE record<person>;
DEFINE FIELD OVERWRITE status ON expunge_request TYPE string DEFAULT 'pending'
    ASSERT $value IN ['pending', 'authorized', 'denied'];
DEFINE FIELD OVERWRITE note ON expunge_request TYPE option<string>;
DEFINE FIELD OVERWRITE resolved_by_person_id ON expunge_request TYPE option<record<person>>;
DEFINE FIELD OVERWRITE expunge_record_id ON expunge_request TYPE option<record<expunge_record>>;
DEFINE FIELD OVERWRITE inserted_at ON expunge_request TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON expunge_request TYPE datetime DEFAULT time::now();
-- The lawyer queue reads `status`; the client-facing "already requested?"
-- check reads `asset_id`. Both Postgres indexes carry over.
DEFINE INDEX OVERWRITE expunge_request_project_status ON expunge_request FIELDS project_id, status;
DEFINE INDEX OVERWRITE expunge_request_asset ON expunge_request FIELDS asset_id;

-- `cases`: one litigation matter before one forum. **Many per Project** —
-- an engagement can carry several cases, differentiated per case and per
-- pleading type, so this is a plain link and not a one-to-one. A `filing`
-- is a notation-scoped compliance record, not a case.
DEFINE TABLE IF NOT EXISTS case SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON case TYPE record<project>;
DEFINE FIELD OVERWRITE caption ON case TYPE string;
DEFINE FIELD OVERWRITE forum ON case TYPE option<string>;
DEFINE FIELD OVERWRITE jurisdiction ON case TYPE option<string>;
DEFINE FIELD OVERWRITE docket_number ON case TYPE option<string>;
DEFINE FIELD OVERWRITE judge ON case TYPE option<string>;
-- Which side the client is on. Free text under Postgres and free text
-- here: the postures worth naming vary by forum, and a closed list would
-- block recording a real one.
DEFINE FIELD OVERWRITE posture ON case TYPE string;
DEFINE FIELD OVERWRITE status ON case TYPE string DEFAULT 'open'
    ASSERT $value IN ['open', 'stayed', 'closed'];
DEFINE FIELD OVERWRITE inserted_at ON case TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON case TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE case_project ON case FIELDS project_id;

-- `case_docket_entries`: the generic case spine, modelled on how a court's
-- own docket works — a numbered list of typed entries. Every case record
-- hangs off this, so a new niche instrument type is one `kind` value
-- rather than a schema change.
--
-- `entry_number` is a **string**: real dockets use attachment sub-numbers
-- such as `29-1`, and the entry list must reference them exactly.
--
-- An absent `document_asset_id` is a **meaningful state**, not an error —
-- the entry renders as *source pending*, derived from whether the link
-- exists rather than from a hand-maintained flag.
DEFINE TABLE IF NOT EXISTS case_docket_entry SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE case_id ON case_docket_entry TYPE record<case>;
DEFINE FIELD OVERWRITE entry_number ON case_docket_entry TYPE string;
DEFINE FIELD OVERWRITE kind ON case_docket_entry TYPE string
    ASSERT $value IN ['pleading', 'motion', 'opposition', 'reply', 'order', 'notice',
                      'stipulation', 'discovery_request', 'discovery_response', 'subpoena',
                      'expert_disclosure', 'hearing', 'trial', 'appeal', 'settlement', 'other'];
DEFINE FIELD OVERWRITE title ON case_docket_entry TYPE string;
DEFINE FIELD OVERWRITE party ON case_docket_entry TYPE option<string>;
DEFINE FIELD OVERWRITE filed_or_served_on ON case_docket_entry TYPE option<string>;
DEFINE FIELD OVERWRITE document_asset_id ON case_docket_entry TYPE option<record<asset>>;
DEFINE FIELD OVERWRITE notation_id ON case_docket_entry TYPE option<record<notation>>;
DEFINE FIELD OVERWRITE inserted_at ON case_docket_entry TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON case_docket_entry TYPE datetime DEFAULT time::now();
-- A docket cannot have two number 1s, but the same number on another case
-- is fine — the uniqueness is per case, exactly as Postgres had it.
DEFINE INDEX OVERWRITE case_docket_entry_case_number ON case_docket_entry
    FIELDS case_id, entry_number UNIQUE;

-- `discovery_requests`: one served set of written discovery — the first
-- device to earn its own table under the flexibility rule. A device lives
-- on the `case_docket_entry` spine alone until it needs internal
-- structure, and a set of numbered items with per-item responses and
-- objections outgrows a single spine row.
--
-- `responses_due_on` records what the served papers say. The Deadlines
-- module answers what is *due*; this row records what exists.
DEFINE TABLE IF NOT EXISTS discovery_request SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE case_id ON discovery_request TYPE record<case>;
-- Optional: a device is keyed to the case, and only optionally to the
-- entry recording its service.
DEFINE FIELD OVERWRITE docket_entry_id ON discovery_request TYPE option<record<case_docket_entry>>;
DEFINE FIELD OVERWRITE device ON discovery_request TYPE string
    ASSERT $value IN ['interrogatories', 'requests_for_production', 'requests_for_admission'];
DEFINE FIELD OVERWRITE set_number ON discovery_request TYPE int;
DEFINE FIELD OVERWRITE propounding_party ON discovery_request TYPE string;
DEFINE FIELD OVERWRITE responding_party ON discovery_request TYPE string;
DEFINE FIELD OVERWRITE served_on ON discovery_request TYPE option<string>;
DEFINE FIELD OVERWRITE responses_due_on ON discovery_request TYPE option<string>;
DEFINE FIELD OVERWRITE status ON discovery_request TYPE string DEFAULT 'served'
    ASSERT $value IN ['served', 'responded', 'closed'];
DEFINE FIELD OVERWRITE inserted_at ON discovery_request TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON discovery_request TYPE datetime DEFAULT time::now();
-- One set number per device per case, so "Set Two" is unambiguous — while
-- a different device may reuse the number.
DEFINE INDEX OVERWRITE discovery_request_case_device_set ON discovery_request
    FIELDS case_id, device, set_number UNIQUE;

-- `discovery_items`: one numbered item within a served set, carrying the
-- propounded request verbatim, the response once drafted, and any
-- objections asserted to it.
DEFINE TABLE IF NOT EXISTS discovery_item SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE discovery_request_id ON discovery_item TYPE record<discovery_request>;
DEFINE FIELD OVERWRITE item_number ON discovery_item TYPE int;
DEFINE FIELD OVERWRITE request_text ON discovery_item TYPE string;
DEFINE FIELD OVERWRITE response_text ON discovery_item TYPE option<string>;
DEFINE FIELD OVERWRITE objections ON discovery_item TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON discovery_item TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON discovery_item TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE discovery_item_request_number ON discovery_item
    FIELDS discovery_request_id, item_number UNIQUE;

-- `disclosures`: formal disclosures attached to an Entity or a matter —
-- conflicts, related-party, and the rest. Both links are optional
-- (a disclosure can be about a company, about a matter, or neither), so
-- neither is the scoping key and the table carries no index beyond the
-- entity lookup the conflict graph runs.
DEFINE TABLE IF NOT EXISTS disclosure SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE entity_id ON disclosure TYPE option<record<entity>>;
DEFINE FIELD OVERWRITE project_id ON disclosure TYPE option<record<project>>;
-- The Postgres table carried no CHECK on `kind`, and this one carries no
-- ASSERT: the conflict graph reads two kinds (`conflict`,
-- `related_party`) but the table records any formal disclosure the firm
-- makes, and a closed list here would block recording a real one.
DEFINE FIELD OVERWRITE kind ON disclosure TYPE string;
DEFINE FIELD OVERWRITE summary ON disclosure TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON disclosure TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON disclosure TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE disclosure_entity ON disclosure FIELDS entity_id;

-- `communications`: one message in a matter's single privileged
-- conversation log, whatever door it arrived through. The **spine** of a
-- spine+satellites model (not single-table inheritance): this row carries
-- what every channel shares, while channel-specific fidelity lives in
-- satellites linking back here — the comment anchor in
-- `document_comment`, the RFC headers in the email tables. `channel` is
-- the discriminator, so a new transport costs one ASSERT entry rather
-- than a new field.
--
-- Privilege is structural: every row is project-scoped client
-- communication, read only through `project_id` and never firm-wide.
-- `direction = 'internal'` rows are firm work product and never reach a
-- client — `store::communications::for_project_client_visible` excludes
-- them in the query rather than leaving it to a template.
DEFINE TABLE IF NOT EXISTS communication SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE project_id ON communication TYPE record<project>;
DEFINE FIELD OVERWRITE channel ON communication TYPE string
    ASSERT $value IN ['document_comment', 'email_inbound', 'email_outbound',
                      'portal_message', 'sms_inbound', 'sms_outbound'];
DEFINE FIELD OVERWRITE direction ON communication TYPE string
    ASSERT $value IN ['inbound', 'outbound', 'internal'];
DEFINE FIELD OVERWRITE author_person_id ON communication TYPE option<record<person>>;
DEFINE FIELD OVERWRITE counterparty ON communication TYPE option<string>;
DEFINE FIELD OVERWRITE subject ON communication TYPE option<string>;
-- The only body representation: every message is plain text, whether it
-- arrived over a transport or was typed into the portal composer.
DEFINE FIELD OVERWRITE body ON communication TYPE string;
DEFINE FIELD OVERWRITE source_ref ON communication TYPE option<string>;
DEFINE FIELD OVERWRITE asset_id ON communication TYPE option<record<asset>>;
-- When the message actually happened, distinct from when the row landed.
DEFINE FIELD OVERWRITE occurred_at ON communication TYPE string;
DEFINE FIELD OVERWRITE inserted_at ON communication TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON communication TYPE datetime DEFAULT time::now();
-- The thread view reads a whole matter oldest-first.
DEFINE INDEX OVERWRITE communication_project_occurred ON communication
    FIELDS project_id, occurred_at;
-- Idempotent ingest: a re-delivered email or re-ingested source cannot
-- duplicate. An absent `source_ref` does NOT collide — many rows may carry
-- NONE under one unique index — so a channel with no external id (a
-- document comment) stays unrestricted, exactly as the Postgres index
-- behaved. `store::communications::ingest` covers both halves.
DEFINE INDEX OVERWRITE communication_channel_source_ref ON communication
    FIELDS channel, source_ref UNIQUE;

-- `email_conversations`: one threaded support exchange behind the firm's
-- support address (the "headless Front"). Keyed by an opaque `token` that
-- rides in the `Reply-To` of every hop, so lawyer and client replies thread
-- back here without leaking an internal address. `status` projects from
-- the latest message; `notation_id`, when set, links the thread to a
-- running matter so an attorney's `@approve` reply can signal a workflow.
DEFINE TABLE IF NOT EXISTS email_conversation SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE token ON email_conversation TYPE string;
DEFINE FIELD OVERWRITE external_email ON email_conversation TYPE string;
DEFINE FIELD OVERWRITE external_name ON email_conversation TYPE option<string>;
DEFINE FIELD OVERWRITE person_id ON email_conversation TYPE option<record<person>>;
DEFINE FIELD OVERWRITE subject ON email_conversation TYPE string;
DEFINE FIELD OVERWRITE status ON email_conversation TYPE string DEFAULT 'open'
    ASSERT $value IN ['open', 'awaiting_lawyer', 'awaiting_client', 'closed'];
-- Now a real link: `notations` and this table are both SurrealDB tables
-- since wave six, so the thread-to-matter join is enforced rather than an
-- unenforced cross-engine id.
DEFINE FIELD OVERWRITE notation_id ON email_conversation TYPE option<record<notation>>;
DEFINE FIELD OVERWRITE inserted_at ON email_conversation TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON email_conversation TYPE datetime DEFAULT time::now();
-- The token is the routing key every inbound hop is resolved by, so a
-- collision would thread two exchanges together.
DEFINE INDEX OVERWRITE email_conversation_token ON email_conversation FIELDS token UNIQUE;
DEFINE INDEX OVERWRITE email_conversation_external_email ON email_conversation
    FIELDS external_email;

-- `email_conversation_messages`: the append-only transcript of a support
-- thread. One row per hop, never updated in place — the parent
-- conversation's `status` projects from the latest row.
DEFINE TABLE IF NOT EXISTS email_conversation_message SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE conversation_id ON email_conversation_message
    TYPE record<email_conversation>;
DEFINE FIELD OVERWRITE direction ON email_conversation_message TYPE string
    ASSERT $value IN ['from_external', 'to_lawyer', 'from_lawyer', 'to_external', 'system'];
DEFINE FIELD OVERWRITE from_addr ON email_conversation_message TYPE string;
DEFINE FIELD OVERWRITE to_addr ON email_conversation_message TYPE string;
DEFINE FIELD OVERWRITE subject ON email_conversation_message TYPE string;
-- Cleaned body: quoted history and signature are stripped from lawyer
-- replies, so a relayed message carries only the new prose.
DEFINE FIELD OVERWRITE body_text ON email_conversation_message TYPE string;
DEFINE FIELD OVERWRITE raw_storage_key ON email_conversation_message TYPE option<string>;
DEFINE FIELD OVERWRITE provider_message_id ON email_conversation_message TYPE option<string>;
DEFINE FIELD OVERWRITE in_reply_to ON email_conversation_message TYPE option<string>;
-- Parsed lawyer-reply directives (`@approve`/`@deny`/…) as a JSON string;
-- absent when the reply carried no command.
DEFINE FIELD OVERWRITE command_payload ON email_conversation_message TYPE option<string>;
DEFINE FIELD OVERWRITE inserted_at ON email_conversation_message TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON email_conversation_message TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE email_conversation_message_conversation ON email_conversation_message
    FIELDS conversation_id;
