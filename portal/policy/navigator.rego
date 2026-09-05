package navigator.authz

import rego.v1

default allow := false

# Owner and Admin are supersets of Lawyer: either privileged tier
# passes every lawyer-work gate. Clerk is deliberately absent; adding
# a non-lawyer role must not inherit legal authority by accident.
lawyer_tier := {"owner", "admin", "lawyer"}
admin_tier := {"owner", "admin"}

is_authenticated(session) if {
    session != null
}

is_lawyer(session) if {
    session != null
    lawyer_tier[session.role]
}

is_clerk(session) if {
    session != null
    session.role == "clerk"
}

is_admin(session) if {
    session != null
    admin_tier[session.role]
}

# Owner/Admin bypass: an authenticated Owner or Admin reaches every route the
# other rules below don't otherwise allow, except `/app/owner`, which is the
# deployment-wide firm inventory and is Owner only. Per docs/access-model.md
# this bypass is silent — no per-read audit row.
owner_only_path if {
    input.path[0] == "app"
    input.path[1] == "owner"
}

allow if {
    is_admin(input.session)
    not owner_only_path
}

allow if {
    owner_only_path
    input.session.role == "owner"
}

# /app/projects/* is the one matter surface (ENG-81). Every tier enters the
# same path, Clerk included now that the dedicated `/clerk` namespace is
# retired, so the policy cannot make the firm/client/supervised split here —
# and must not try. It admits any authenticated caller; which of the five
# renderings a caller gets, and whether they may see this matter at all, is
# decided in the handler by `store::access::matter_viewer`, the only layer that
# can read the participation ledger.
#
# The Clerk boundary that the separate `/clerk` mount used to carry
# structurally is now carried by that resolver: a Clerk resolves to
# `MatterViewer::Clerk` only when they hold a firm-side row *and* the matter's
# flagged lawyer DRI currently holds the lawyer tier, and that variant renders
# the name/status/supervisor page — never documents or legal work.
#
# This is deliberately weaker than the `/app/lawyer/*` rule these paths used to sit
# behind. The lawyer-only writes underneath — matter open/edit/delete, the
# participation forms, document upload, transcript intake — each re-check the
# lawyer tier in their own handler, which is what carries that guard now. A
# route added here without its own tier check is admitting every signed-in
# client.
allow if {
    input.path[0] == "app"
    input.path[1] == "projects"
    is_authenticated(input.session)
}

# /app/lawyer is the lawyer workbench: the firm dashboard and every
# remaining lawyer-tier listing and walk that used to sit under a
# separate `/lawyer` prefix. Owner and Admin reach it through the
# route bypass above; this prefix rule is what admits Lawyer. Clerk
# and client are denied here, which keeps the CRUD directory and
# legal-work surfaces away from a supervised non-lawyer.
allow if {
    input.path[0] == "app"
    input.path[1] == "lawyer"
    is_lawyer(input.session)
}

# Reconcile one committed pointer's visibility against its asset. The handler
# repeats the lawyer-tier matter-scope check and collapses an out-of-scope asset
# to 404. Six segments distinguishes this exact asset command from collection
# upload and all other Project subpaths.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "documents"
    count(input.path) == 6
    input.method == "PATCH"
    is_lawyer(input.session)
}

# /app/outline is the bundled Harvard-outline recording stage. Lawyer-tier
# only. A notation the firm has given a client is a different page
# (`/app/projects/{code}/{notation_id}/outline`) and rides the matter-surface
# rule above; this catalog is firm teaching material, not a client's letter.
allow if {
    input.path == ["app", "outline"]
    is_lawyer(input.session)
}

# /app/docs is the workspace documentation inside the application. It admits
# every tier that operates Navigator — Lawyer and Clerk by the two rules
# below, Owner and Admin through the route bypass at the top of this policy.
#
# `client` is the one authenticated tier denied here. These documents describe
# how the firm runs the product, not anything a client does.
#
# Note what this does and does not change. `/docs` carries no rule in this
# policy and is not behind the session boundary either: it is an anonymous
# public surface, because the repository is source-available and those documents
# are the manual for software anyone can clone. `/app/docs` is therefore not a
# gate over the documents at all — it is a second door to the same index
# wearing the application chrome, and what it restricts is that surface.
#
# Clerk is admitted by an explicit rule rather than by widening `lawyer_tier`:
# per the note at the top of this file, a non-lawyer role must never inherit
# legal authority as a side effect of being added somewhere. Reading the docs
# is not legal authority, so Clerk gets its own rule and `lawyer_tier` is
# untouched.
# The prefix match covers the hub and every document beneath it, because both
# carry the same audience — there is no document in the index that a reader
# admitted to the hub may not read.
allow if {
    input.path[0] == "app"
    input.path[1] == "docs"
    is_lawyer(input.session)
}

allow if {
    input.path[0] == "app"
    input.path[1] == "docs"
    is_clerk(input.session)
}

# /app/team is the firm team's home. It admits every firm tier and denies the
# client tier, which uses the Project home instead.
#
# Same audience as `/app/docs`, and admitted the same way: Lawyer and Clerk by
# the two rules below, Owner and Admin through the route bypass at the top of
# this policy. `client` is the one authenticated tier denied.
#
# Clerk gets an explicit rule rather than being folded into `lawyer_tier`, for
# the reason stated at the top of this file: a non-lawyer role must never
# inherit legal authority as a side effect of being admitted somewhere.
# Downloading the CLI is not legal authority.
#
# These two rules are a prefix, so they also carry the page's own assets —
# `/app/team/fonts/gorp-serif.zip`, the licensed GORP Serif desktop family the
# home offers as a card. That download is a firm brand asset, not lawyer work,
# and its audience is exactly this page's: all four firm tiers, no client. It
# used to sit under `/app/lawyer`, where admitting a Clerk needed an exact-path
# exception to "Clerk never enters /app/lawyer"; here it needs no rule of its own.
#
allow if {
    input.path[0] == "app"
    input.path[1] == "team"
    is_lawyer(input.session)
}

allow if {
    input.path[0] == "app"
    input.path[1] == "team"
    is_clerk(input.session)
}

# /app/brands is the house-of-brands home: every registered brand's typeface.
# Same audience as /app/team, admitted the same way: Lawyer and Clerk by the
# two rules below, Owner and Admin through the route bypass at the top of this
# policy. A brand's font is a firm brand asset, not lawyer work — a Clerk gets
# its own rule rather than being folded into `lawyer_tier`, for the reason
# stated at the top of this file.
allow if {
    input.path[0] == "app"
    input.path[1] == "brands"
    is_lawyer(input.session)
}

allow if {
    input.path[0] == "app"
    input.path[1] == "brands"
    is_clerk(input.session)
}

# /app/admin is Owner/Admin only at the hub, the matter directory
# (`/app/admin/projects`), Person CRUD (`/app/admin/people`), and visitor
# analytics. Those need no rule of their own: the route bypass at the top of
# this policy is exactly that set. Spelled out here as a deny-by-omission note
# rather than a rule, because a prefix `is_lawyer` grant for `/app/admin` would
# silently widen the hub and the matter directory to Lawyer.
#
# Firm-administration listings that a Lawyer already reached under `/app/lawyer`
# now live as named resources under `/app/admin`. The grant is the resource
# segment, not the prefix, so `/app/admin/people` and `/app/admin/projects`
# stay Owner/Admin. Letters and the email log are in the set so admission
# matches the old `/app/lawyer` prefix; their handlers still require the admin
# tier.
admin_lawyer_resources := {
	"letters",
	"email-log",
	"addresses",
	"entities",
	"entity-types",
	"git-repositories",
	"jurisdictions",
	"mailrooms",
	"playbooks",
	"questions",
	"templates",
	"schedules",
	"people.csv",
}

allow if {
	input.path[0] == "app"
	input.path[1] == "admin"
	count(input.path) >= 3
	admin_lawyer_resources[input.path[2]]
	is_lawyer(input.session)
}

# /app/notations/:id/documents/:doc_id exposes reviewed notation
# PDFs through the client lens. The handler resolves the notation to
# its Project and applies the client-lens project ACL before issuing
# any signed URL.
allow if {
    input.path[0] == "app"
    input.path[1] == "notations"
    is_authenticated(input.session)
}

# /app/forms/* — blank government forms (public records,
# vendored from each authority's own site). Any authenticated
# person may browse the index and download a blank; the *filled*
# packets are client documents and live on project routes, never
# here.
allow if {
    input.path[0] == "app"
    input.path[1] == "forms"
    is_authenticated(input.session)
}

# Workshop reads are public and never reach this policy. Claiming a completion
# certificate remains a firm-side action for Lawyer and Clerk; Owner and Admin
# reach it through the bypass above.
allow if {
    input.path[0] == "workshops"
    count(input.path) == 3
    input.path[2] == "certificate"
    input.method == "POST"
    is_lawyer(input.session)
}

allow if {
    input.path[0] == "workshops"
    count(input.path) == 3
    input.path[2] == "certificate"
    input.method == "POST"
    is_clerk(input.session)
}

# MCP (LibreChat-driven tool calls) requires a Lawyer or admin —
# the tools mutate the same CRM tables.
allow if {
    input.path[0] == "mcp"
    is_lawyer(input.session)
}

# A2A JSON-RPC dispatches to the same MCP tool registry — same
# lawyer-tier requirement. The agent card at /app/api/aida.json is
# decided at the router level, not here: it composes behind the session
# boundary alone, because reading the card is not a tool call. So this
# rule governs only the RPC endpoint, the path that actually dispatches.
allow if {
    input.path == ["app", "api", "aida", "rpc"]
    is_lawyer(input.session)
}

# The API documentation surfaces — the Swagger UI shell at /app/api (and its
# short public alias /api) and the OpenAPI document beside it — carry no rule
# here at all. `portal::bootstrap` mounts them with no session boundary and no
# `require_policy` layer, so this policy never sees a request for them: the
# reference is public, and a reader needs no session just to see what the API
# looks like. What the reference *describes* is a different set of paths
# entirely — `/app/api/people` and the rest of the data reads below — and
# those keep their own per-resource rules unchanged. A Clerk (or an anonymous
# reader) sees the reference but not the directory it describes.
#
# ---------------------------------------------------------------------------
# /app/api read paths — one rule per resource, no blanket grant.
# ---------------------------------------------------------------------------
#
# There used to be a single rule here admitting any authenticated caller to
# every `GET /app/api/*` path. It was the only grant on this surface that named
# no resource, and it had two consequences:
#
# 1. A `client` read the firm's whole people and entities directory. The
#    handlers carry no tier check of their own on the read path, so this rule
#    was the only thing standing there, and it said yes.
# 2. A newly added GET route was authorized the moment it was routed. Adding a
#    read endpoint could not fail closed, because the grant did not depend on
#    anyone having considered it.
#
# Each read is now named with the tier that should have it. A new GET route
# under /app/api gets no decision until a rule is written for it, which is the
# behaviour a default-deny policy is supposed to have.

# The firm's CRM directory: people and entities, collection and item alike.
# Lawyer and admin only. This is the firm's own book of clients,
# counterparties, and contacts — a client sees their own matter through the
# project surfaces, never the directory those surfaces are drawn from.
#
# The prefix match covers `/app/api/people` and `/app/api/people/{id}`
# together, because "who may list the directory" and "who may read one row of
# it" are the same question here.
api_directory_resources := {"people", "entities"}

allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    api_directory_resources[input.path[2]]
    input.method == "GET"
    is_lawyer(input.session)
}

# Reference data: the jurisdictions list and the entity-type vocabulary. Both
# are firm-side authoring inputs — they populate the lawyer matter and entity
# forms — and neither is reachable from a client surface, so they take the same
# lawyer gate as the directory rather than a wider one. They carry no client
# content, so this is about keeping the surface coherent rather than about
# secrecy: a read a client cannot use is a read a client should not have.
api_reference_resources := {"jurisdictions", "entity-types"}

allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    api_reference_resources[input.path[2]]
    input.method == "GET"
    is_lawyer(input.session)
}

# Raw Template markdown: GET /app/api/templates/*path serves the source of a
# notation template. Deliberately the permissive rule — any authenticated
# person — because it is what the notation gallery at /notations and the
# Template gallery at /templates already link, and both admit every
# authenticated tier including `client`.
#
# This rule changes nothing about who reads it; before this the route simply
# had no policy layer at all and rested on the session boundary. Stating the
# grant is the point: the next reader can see that "any authenticated person"
# was decided rather than defaulted, and tightening it is now one edit here
# plus the two galleries that link it.
#
# Scoped to GET. POST /app/api/templates/validate is the lawyer-tier authoring
# command below and must not be widened by this.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "templates"
    input.method == "GET"
    is_authenticated(input.session)
}

# Matter read clusters (#866). The portal's matter pages read these, and both a
# client (their own matter) and the firm reach them, so they are admitted at the
# session boundary and the handler self-scopes: `visible_projects` returns only
# the caller's matters, and the by-id reads collapse an out-of-scope resource to
# 404. Scoped to GET so the write verbs on these paths keep their own tier rules.
#
# GET /app/api/projects (list), /app/api/projects/{id} (detail),
# /app/api/projects/{id}/{participants,notations} (matter sub-reads).
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.method == "GET"
    count(input.path) <= 5
    is_authenticated(input.session)
}

# GET /app/api/notations/{id} — one notation, scoped by its matter in the handler.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    count(input.path) == 4
    input.method == "GET"
    is_authenticated(input.session)
}

# Firm-tool reads: the contract-review playbooks and one inbound-contract review.
# Not reachable from a client surface, so they take the lawyer tier like the
# directory reads. GET /app/api/playbooks, /app/api/playbooks/{id},
# /app/api/contract-reviews/{id}.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "playbooks"
    input.method == "GET"
    is_lawyer(input.session)
}

allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "contract-reviews"
    count(input.path) == 4
    input.method == "GET"
    is_lawyer(input.session)
}

# The pending client-deletion queue and a notation's review drafts are firm work
# product, so they take the lawyer tier. GET /app/api/expunge-requests and
# /app/api/notations/{id}/review-documents. (A matter's documents and
# conversation are read under the `projects` GET rule above, client-visible-
# filtered in the handler.)
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "expunge-requests"
    count(input.path) == 3
    input.method == "GET"
    is_lawyer(input.session)
}

allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "review-documents"
    count(input.path) == 5
    input.method == "GET"
    is_lawyer(input.session)
}

# Stateless Template markdown validator: POST /app/api/templates/validate
# lints draft markdown and returns violations. It touches no database,
# but linting a Template is a lawyer authoring activity — so it takes
# the same lawyer-tier gate as every other /app/api/* command, not the
# any-authenticated GET grant. The canonical path is
# /app/api/templates/validate; there is no /app/api/notations/validate alias.
allow if {
    input.path == ["app", "api", "templates", "validate"]
    input.method == "POST"
    is_lawyer(input.session)
}

# People command resource: create/update/delete and the welcome-send
# command move through POST/PATCH/DELETE /app/api/people*. These are the
# REST command boundary for both the browser lawyer forms (cookie +
# CSRF) and machine clients (bearer). Owner/Admin/Lawyer only — the
# handler re-checks the tier, but the policy gate is the first line so a
# client or anonymous write never reaches it. Read paths stay open to
# any authenticated caller via the GET rule above.
api_write_methods := {"POST", "PATCH", "DELETE"}

allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "people"
    api_write_methods[input.method]
    is_lawyer(input.session)
}

# Entity command resource: POST /app/api/entities is the REST command
# boundary the lawyer create form, the inline "Add entity" modal on the
# project form, and machine clients all travel. Same lawyer-tier gate as
# the People commands — a client or anonymous write never reaches the
# handler's own check. Read paths stay open to any authenticated caller
# via the GET rule above.
#
# The entities resource is now complete on the command boundary: create,
# update, and delete all have handlers, so this grant covers the full
# `api_write_methods` set. Each verb was added here in the same change
# that added its handler, never ahead of it.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "entities"
    api_write_methods[input.method]
    is_lawyer(input.session)
}

# Seed reconciliation: POST /app/api/seed accepts the same YAML envelope as
# the checked-in seed files. The handler maps its model name through the typed
# seed registry, so this rule grants a command rather than arbitrary database
# access. It is lawyer-tier for the same reason as directory writes.
allow if {
    input.path == ["app", "api", "seed"]
    input.method == "POST"
    is_lawyer(input.session)
}

# Upload a contract for playbook review: POST /app/api/projects/{id}/contract-review
# ingests an inbound third-party contract (multipart) and runs the deviation
# analysis. Client-writable like the review surface — a matter's client may
# submit their own contract, or the firm may — so any authenticated caller is
# admitted; the handler then enforces matter scope through either lens (lawyer
# or client), collapsing a non-participant to 404. Scoped to the
# contract-review sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "contract-review"
    input.method == "POST"
    is_authenticated(input.session)
}

# Add-participant command: POST /app/api/projects/{id}/participants adds a person
# to a matter's participation ledger (co-counsel, paralegal, a second client
# contact, or a Clerk being granted portal visibility). Firm-side
# matter-management action, so lawyer/admin only; a client or anonymous write
# never reaches the handler's own LawyerSession check. The handler additionally
# re-checks that the acting lawyer participates in the matter (admin bypasses),
# same as `/close` below, and collapses an out-of-scope matter to 404. Scoped to
# the participants sub-path and POST, the one verb this slice ships — edit/remove
# earn their own grants when they land.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "participants"
    input.method == "POST"
    is_lawyer(input.session)
}

# Open a matter's closing-letter notation: POST /app/api/projects/{id}/close is
# the REST mirror of the lawyer close control. Firm-side matter action,
# lawyer/admin only at the tier; the handler additionally re-checks that the
# acting lawyer participates in the matter (admin bypasses) and collapses an
# out-of-scope matter to 404. Scoped to the close sub-path (five segments) and
# POST, the one verb it ships.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "close"
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# Move a matter directly through its lifecycle (close/reopen/archive): POST
# /app/api/projects/{id}/lifecycle reaches `store::projects::transition_project`
# directly — no closing-letter notation opens here, unlike `/close` above.
# Lawyer/admin only at the tier, same as the bare PATCH/DELETE matter path
# below: a lifecycle move is firm administration, not scoped to the acting
# lawyer's own matters. Scoped to the lifecycle sub-path (five segments) and
# POST, the one verb it ships.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "lifecycle"
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# Approve a released estate plan: POST /app/api/projects/{id}/approve-plan is
# the client approving their own plan, so — unlike every other project write —
# it is client-writable. Any authenticated caller is admitted here; the handler
# enforces client-lens matter access (a caller who is not this matter's client
# sees 404). Scoped to the approve-plan sub-path (five segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "approve-plan"
    count(input.path) == 5
    input.method == "POST"
    is_authenticated(input.session)
}

# Post a matter conversation message: POST
# /app/api/projects/{id}/conversation/messages. Both the client and the firm
# post here, so it is client-writable. Any authenticated caller is admitted; the
# handler enforces either-lens matter access (a non-participant sees 404) and the
# tier decides the message side. Scoped to the conversation/messages sub-path
# (six segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "conversation"
    input.path[5] == "messages"
    count(input.path) == 6
    input.method == "POST"
    is_authenticated(input.session)
}

# Reconcile every matter against the repository it records: GET
# /app/api/project-repositories. Admin-only, and it carries its own noun rather
# than sitting under `projects` on purpose — the projects GET rule above admits
# any authenticated caller up to five segments, so nesting this there would make
# it policy-reachable by a client. Three segments and GET.
allow if {
	input.path[0] == "app"
	input.path[1] == "api"
	input.path[2] == "project-repositories"
	count(input.path) == 3
	input.method == "GET"
	is_admin(input.session)
}

# Read every matter's lifecycle fields: GET /app/api/project-lifecycle.
# Admin-only, and it carries its own noun because the projects GET rule admits
# any authenticated caller up to five segments. Three segments and GET.
allow if {
	input.path[0] == "app"
	input.path[1] == "api"
	input.path[2] == "project-lifecycle"
	count(input.path) == 3
	input.method == "GET"
	is_admin(input.session)
}

# Create or adopt a Project's Drive ingest folder and source repository:
# POST /app/api/project-surfaces/{id}. Admin-only, and it carries its own
# noun rather than sitting under `projects` on purpose — the projects GET
# rule above admits any authenticated caller up to five segments, so nesting
# this there would make it policy-reachable by a client. Four segments and
# POST.
allow if {
	input.path[0] == "app"
	input.path[1] == "api"
	input.path[2] == "project-surfaces"
	count(input.path) == 4
	input.method == "POST"
	is_admin(input.session)
}

# Authorize a client document-deletion request: POST
# /app/api/expunge-requests/{id}/authorize runs the governed expunge, so it is
# admin-only — the one write on this surface that the lawyer tier alone cannot
# fire. Scoped to the authorize sub-path (five segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "expunge-requests"
    input.path[4] == "authorize"
    count(input.path) == 5
    input.method == "POST"
    is_admin(input.session)
}

# Deny a client document-deletion request: POST
# /app/api/expunge-requests/{id}/deny resolves it without deleting anything, so
# the lawyer tier may fire it. Scoped to the deny sub-path (five segments) and
# POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "expunge-requests"
    input.path[4] == "deny"
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# Create a contract-review playbook: POST /app/api/playbooks. A firm tool for
# authoring a Company's negotiating positions; lawyer/admin only. Scoped to the
# playbooks collection (three segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "playbooks"
    count(input.path) == 3
    input.method == "POST"
    is_lawyer(input.session)
}

# Replace a playbook's positions: PUT /app/api/playbooks/{id}. Same firm tool;
# lawyer/admin only. Scoped to the playbook item (four segments) and PUT.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "playbooks"
    count(input.path) == 4
    input.method == "PUT"
    is_lawyer(input.session)
}

# The contract-review workbench: save a finding, edit the summary, approve, or
# reject an inbound-contract review. Firm-side matter action, lawyer/admin only
# at the tier; each handler additionally re-checks the acting lawyer participates
# in the review's matter (admin bypasses) and collapses out-of-scope to 404.
# Save a finding: POST /app/api/contract-reviews/{id}/findings/{idx} (six segs).
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "contract-reviews"
    input.path[4] == "findings"
    count(input.path) == 6
    input.method == "POST"
    is_lawyer(input.session)
}

# Edit the summary, approve, or reject: POST
# /app/api/contract-reviews/{id}/{summary,approve,reject} (five segments).
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "contract-reviews"
    input.path[4] in {"summary", "approve", "reject"}
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# File a document into a matter: POST /app/api/projects/{id}/documents. Firm-side
# matter write, lawyer/admin only; the handler re-checks matter participation and
# collapses out-of-scope to 404. Scoped to the documents sub-path (five segments)
# and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "documents"
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# Run a transcript against a notation's questionnaire: POST
# /app/api/notations/{id}/transcript. Firm-side matter action, lawyer/admin only;
# the handler re-checks matter participation. Scoped to the transcript sub-path
# under notations (five segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "transcript"
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# File an estate matter's sitting transcript: POST
# /app/api/projects/{id}/notations/{nid}/transcript. Firm-side matter action,
# lawyer/admin only; the handler re-checks participation and that the notation
# belongs to the matter. Scoped to the transcript sub-path under a project's
# notation (seven segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "notations"
    input.path[6] == "transcript"
    count(input.path) == 7
    input.method == "POST"
    is_lawyer(input.session)
}

# Edit + remove a participation row: PATCH/DELETE
# /app/api/projects/{id}/participants/{role_id} rewrite or drop one participant
# (the command refuses to strand the matter's lawyer DRI) — this is also how a
# participating lawyer revokes a Clerk's portal visibility. Same firm-side
# matter-management action as the add above; lawyer/admin only at the tier, and
# the handler re-checks matter participation the same way. Scoped to the
# participant-item path (five segments) and the two mutating verbs.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "participants"
    count(input.path) == 6
    input.method in {"PATCH", "DELETE"}
    is_lawyer(input.session)
}

# Designate or clear a participant's DRI marker: PUT/DELETE
# /app/api/projects/{id}/participants/{role_id}/dri. Same firm-side matter
# self-governance as the edit/remove above; lawyer/admin only at the tier. This
# grant is necessary but not sufficient — the command additionally gates the
# change on the caller already holding the marker for that side, so a lawyer who
# is not a current DRI is admitted here and refused by the command. Scoped to the
# dri sub-path (seven segments) and the two mutating verbs.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "participants"
    input.path[6] == "dri"
    count(input.path) == 7
    input.method in {"PUT", "DELETE"}
    is_lawyer(input.session)
}

# Open a notation on a matter: POST /app/api/projects/{id}/notations opens a
# notation from a template authored in the matter's own repo (or the bundled
# firm catalog). Firm-side matter action, lawyer/admin only at the
# tier; this lawyer grant is necessary but not sufficient, because the handler
# additionally re-checks that the acting lawyer participates in the matter
# (admin bypasses) and collapses an out-of-scope matter to 404. Scoped to the
# notations sub-path and POST, the one verb this slice ships.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    input.path[4] == "notations"
    input.method == "POST"
    is_lawyer(input.session)
}

# Answer a notation's questionnaire step: POST /app/api/notations/{id}/answers
# records the answer to the step the questionnaire is currently asking,
# attributed to the acting lawyer. Firm-side matter action, lawyer/admin
# only at the tier; the handler additionally re-checks that the acting lawyer
# participates in the notation's matter (admin bypasses) and collapses an
# out-of-scope notation to 404. The client-facing self-serve intake (the
# magic-link walk) is a separate surface, not this REST path. Scoped to the
# answers sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "answers"
    input.method == "POST"
    is_lawyer(input.session)
}

# Send a notation back for changes: POST
# /app/api/notations/{id}/request-changes flags answers and fires the
# changes_requested transition. Firm-side matter action, lawyer/admin only at
# the tier; the handler additionally re-checks matter participation (admin
# bypasses) and collapses an out-of-scope notation to 404. Scoped to the
# request-changes sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "request-changes"
    input.method == "POST"
    is_lawyer(input.session)
}

# Resubmit a re-collected notation: POST /app/api/notations/{id}/reask writes the
# re-collected answers and fires intake_resubmitted. Firm-side matter action,
# lawyer/admin only at the tier; the handler additionally re-checks matter
# participation (admin bypasses) and collapses an out-of-scope notation to 404.
# Scoped to the reask sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "reask"
    input.method == "POST"
    is_lawyer(input.session)
}

# Send a notation's client their self-serve intake link: POST
# /app/api/notations/{id}/intake emails the matter's client the magic link that
# backs their intake walk. Firm-side matter action, lawyer/admin only
# at the tier; the handler additionally re-checks that the acting lawyer
# participates in the notation's matter (admin bypasses) and collapses an
# out-of-scope notation to 404. Scoped to the intake sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "intake"
    input.method == "POST"
    is_lawyer(input.session)
}

# Approve a notation parked at lawyer_review: POST /app/api/notations/{id}/approval
# re-assembles the reviewed document and fires the `approved` transition so
# the worker renders the PDF. A binding attorney action (the reviewed paper
# is what goes out for signature next), so lawyer/admin only at the
# tier; the handler additionally re-checks that the acting lawyer participates
# in the notation's matter (admin bypasses) and collapses an out-of-scope
# notation to 404. Scoped to the approval sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "approval"
    input.method == "POST"
    is_lawyer(input.session)
}

# Dispatch a notation for signature: POST /app/api/notations/{id}/signature fires
# `pdf_persisted` and sends exactly one signature envelope. The binding send
# — the reviewed engagement goes out for the client's signature — so lawyer
# lawyer/admin only at the tier; the handler additionally re-checks that the
# acting lawyer participates in the notation's matter (admin bypasses) and
# collapses an out-of-scope notation to 404. Scoped to the signature
# sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "signature"
    input.method == "POST"
    is_lawyer(input.session)
}

# Release an estate notation's drafts to client review: POST
# /app/api/notations/{id}/release-drafts advances lawyer_review -> client_review
# and flips every generated draft instrument to pending_review (making it
# visible to the client). The attorney gate before a client-facing legal
# document leaves `draft`, so lawyer/admin only at the tier; the
# handler additionally re-checks that the acting lawyer participates in the
# notation's matter (admin bypasses) and collapses an out-of-scope notation
# to 404. Scoped to the release-drafts sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "release-drafts"
    input.method == "POST"
    is_lawyer(input.session)
}

# Append a custom clause to a notation's document: POST
# /app/api/notations/{id}/clauses adds firm-authored prose spliced into the
# assembled body at render time. Firm-side document authoring, so lawyer
# lawyer/admin only at the tier; the handler additionally re-checks that the
# acting lawyer participates in the notation's matter (admin bypasses) and
# collapses an out-of-scope notation to 404. Scoped to the clauses sub-path
# (the collection, four segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "clauses"
    count(input.path) == 5
    input.method == "POST"
    is_lawyer(input.session)
}

# Edit + remove a clause: PATCH/DELETE /app/api/notations/{id}/clauses/{clause_id}
# rewrite or drop one clause. Same firm-side document authoring as the append
# above; lawyer/admin only, and the handler re-checks matter scope and
# that the clause belongs to the notation. Scoped to the clause-item path
# (five segments) and the two mutating verbs.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "clauses"
    count(input.path) == 6
    input.method in {"PATCH", "DELETE"}
    is_lawyer(input.session)
}

# Reorder a clause: POST /app/api/notations/{id}/clauses/{clause_id}/move swaps it
# with its neighbour. Same firm-side authoring action; lawyer/admin
# only. Scoped to the clause move sub-path (six segments) and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "notations"
    input.path[4] == "clauses"
    count(input.path) == 7
    input.path[6] == "move"
    input.method == "POST"
    is_lawyer(input.session)
}

# Add a comment to a review document: POST /app/api/review-documents/{id}/comments.
# The FIRST client-writable /api door — the read-only review surface lets a
# matter's CLIENT annotate the document, so this allows any authenticated
# caller (not just lawyer), mirroring the /app review surface. The handler
# enforces client-lens matter scope (a firm-side-only lawyer or a
# non-participant sees 404) and derives the comment's direction from the
# caller's role. Scoped to the comments sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "review-documents"
    input.path[4] == "comments"
    input.method == "POST"
    is_authenticated(input.session)
}

# Request a document's deletion: POST /app/api/documents/{id}/deletion-requests.
# A matter participant (typically the client) asks for a document to be
# deleted; this only records a pending request — a lawyer/admin must
# authorize the actual expunge. Client-writable like the review surface, so
# any authenticated caller is admitted; the handler enforces client-lens
# matter scope (a firm-side-only lawyer or a non-participant sees 404).
# Scoped to the deletion-requests sub-path and POST.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "documents"
    input.path[4] == "deletion-requests"
    input.method == "POST"
    is_authenticated(input.session)
}

# Project update + delete on the bare matter path:
# PATCH /app/api/projects/{id} edits a matter's descriptive fields (no
# lifecycle/status, no conflict check, no repo provisioning, no price move);
# DELETE /app/api/projects/{id} removes a matter (blocked by the database when
# dependents still reference it). Lawyer/admin only. Scoped to these
# two verbs on the bare matter path — the matter-open (POST) verb earns its
# own grant in the change that adds its handler.
project_bare_methods := {"PATCH", "DELETE"}

allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    count(input.path) == 4
    project_bare_methods[input.method]
    is_lawyer(input.session)
}

# Matter open: POST /app/api/projects opens a new matter — the conflict check,
# the opening attorney's required conflict attestation, and the DRI
# designations. At this firm `lawyer` is an attorney,
# so this lawyer/admin gate is the "an attorney is opening (and attesting)"
# check; a client or anonymous caller never reaches the handler. Scoped to
# POST on the collection path (`/app/api/projects`, length 2), distinct from the
# per-matter verbs above.
allow if {
    input.path[0] == "app"
    input.path[1] == "api"
    input.path[2] == "projects"
    count(input.path) == 3
    input.method == "POST"
    is_lawyer(input.session)
}

# NOTE: the API documentation surfaces (/app/api, its /api alias, and
# /app/api/openapi.json) are NOT decided here. `portal::bootstrap` mounts them
# with no session boundary and no `require_policy` layer at all — see
# `portal::api::doc_routes` — so this policy never evaluates a request for
# them. They are public: a reader needs no session just to see what the API
# looks like, and the document itself carries no client data, only the shape
# of the operations below.
#
# This looks like the same public exemption the #204 incident retired, and
# the difference is what makes it safe again. #204's danger was an
# allow-rule-shaped grant a stale ConfigMap could default-deny out from under
# a path that needed to stay open — the bundle and the `web` image redeploy on
# separate schedules, so a lagging bundle silently narrowed access. A routing
# decision has no such failure mode: there is no policy evaluation on these
# paths to go stale, so a lagging bundle cannot narrow OR widen what they
# serve. The lesson survives; it just never applied to a path this policy
# never sees.
#
# The API those docs describe is a different question, and this file still
# answers it: `/app/api/people` and every other operation below still
# composes behind `portal::auth::require_session`, and this policy still
# decides which authenticated tier proceeds. Reading the reference and
# calling the API it describes are two different gates.
