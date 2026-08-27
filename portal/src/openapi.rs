//! OpenAPI 3.1 description of the JSON `/app/api/*` surface.
//!
//! Hand-curated rather than generated so the schema stays
//! deliberately small and free of toolchain ceremony — each entity
//! we expose ends up as one short `components.schemas` entry. When
//! the API grows enough to justify it, a future commit can swap
//! this for a `utoipa`-derived doc without changing the wire path.
//!
//! Scope: this document covers the JSON endpoints under `/app/api/*`.
//! Every operation requires OIDC authentication — either a
//! browser session cookie (`navigator_session`) issued by the OAuth
//! flow at `/auth/login`, or an upstream-validated JWT / Google OAuth
//! bearer token. The MCP endpoint at `/mcp` is JSON-RPC over HTTP
//! (not REST) and is intentionally NOT documented here — Swagger UI's
//! "Try it out" affordance would mislead callers and risks leaking
//! bearer tokens pasted into a static UI. MCP clients should consult
//! the MCP specification directly.
//!
//! Drift between this document and `api::routes()` is asserted by
//! `web/tests/openapi_drift.rs` at `(method, path)` granularity.
//!
//! # API versioning: `/app/api` is the pre-1.0 contract
//!
//! The surface is mounted unversioned at `/app/api`, not `/app/api/v1`. That is
//! a deliberate, current decision: while the app is pre-1.0 the `/app/api`
//! paths are the working contract and may still change shape as
//! resources move onto the command boundary (issue #355). It is not an
//! oversight and no `/app/api/v1` alias exists.
//!
//! Once the contract is depended on externally, a **breaking** change to
//! any documented request/response (a removed field, a renamed path, a
//! narrowed type) requires an explicit versioning decision — introduce
//! `/app/api/v1` and serve both for a deprecation window — rather than
//! silently changing the wire format under the existing `/app/api` path.
//! Additive, backward-compatible changes (a new optional field, a new
//! endpoint) stay on `/app/api`. This note is the record of that decision so
//! a future contributor does not either bump a version reflexively or,
//! worse, mutate the wire format in place.

use serde_json::{json, Value};

use views::brand::FIRM_BRAND;

/// Documentation placeholder used only when neither an explicit
/// override nor a request host is available. Matches the placeholder
/// substitution flow in `examples/deploy/k8s/gke/`.
const PLACEHOLDER_BASE_URL: &str = "https://www.your-domain.example";

/// Resolve the public-facing base URL for the OpenAPI `servers` and
/// `contact` blocks. Precedence, mirroring how the A2A agent card
/// resolves its authority in [`crate::a2a`]:
///
/// 1. The mounted brand bundle's `base_url`.
/// 2. `NAV_BASE_URL` — a non-brand operational override.
/// 3. The `authority` from the incoming request's `Host` header — so a
///    deploy surfaces its own host (`www.neonlaw.com` in prod) with
///    zero config and no hard-coded domain in source.
/// 4. The documentation placeholder.
#[must_use]
pub fn base_url_for(authority: Option<&str>) -> String {
    if !views::brand::base_url().is_empty() {
        return views::brand::base_url().to_string();
    }
    if let Ok(explicit) = std::env::var("NAV_BASE_URL") {
        if !explicit.is_empty() {
            return explicit;
        }
    }
    match authority.filter(|a| !a.is_empty()) {
        Some(authority) => {
            // Loopback hosts are dev-only and never TLS-terminated.
            let scheme = if authority.starts_with("localhost")
                || authority.starts_with("127.0.0.1")
                || authority.starts_with("0.0.0.0")
            {
                "http"
            } else {
                "https"
            };
            format!("{scheme}://{authority}")
        }
        None => PLACEHOLDER_BASE_URL.to_string(),
    }
}

/// The OpenAPI document with the base URL resolved from env / request
/// host. Convenience wrapper over [`document_with_base`] used by the
/// drift test and unit tests, where no request host is available.
#[must_use]
pub fn document() -> Value {
    document_with_base(&base_url_for(None))
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn document_with_base(base: &str) -> Value {
    let contact_url = format!("{base}/contact");
    json!({
      "openapi": "3.1.0",
      "info": {
        "title": "Neon Law Navigator API",
        "version": "0.1.0",
        "description":
          "JSON listings and commands for the Neon Law Navigator domain tables, plus a stateless \
           markdown notation validator. The whole surface is private and lives under \
           `/app/api`. Every endpoint requires OIDC authentication — either a browser \
           session cookie issued by the OAuth flow at `/auth/login`, or a JWT bearer \
           token — and so does the documentation: this document at \
           `/app/api/openapi.json` and the Swagger UI that renders it at `/app/api` \
           both answer an anonymous caller with an unauthenticated stub rather than the \
           schema. The MCP endpoint at `/mcp` (JSON-RPC, Google OAuth bearer) is \
           documented separately.",
        "contact": { "name": FIRM_BRAND.site_name, "url": contact_url }
      },
      "servers": [
        { "url": base, "description": "Production" }
      ],
      "security": [
        { "bearerAuth": [] },
        { "sessionCookie": [] }
      ],
      "paths": {
        "/app/api/people": {
          "get": {
            "summary": "List all people",
            "x-mcp-tool": "aida_show_person",
            "responses": {
              "200": { "description": "Person list", "content": { "application/json": {
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Person" } }
              } } }
            }
          },
          "post": {
            "summary": "Create a person",
            "x-mcp-tool": "aida_create_person",
            "description":
              "Creates one Person row. The `role` field defaults conservatively to `client`; \
               supported values are `owner`, `admin`, `lawyer`, `clerk`, and `client`, and non-empty invalid \
               values are rejected. Blank structured legal-name fields are stored as null. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin`; \
               anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/CreatePersonRequest" }
              } }
            },
            "responses": {
              "201": { "description": "Created person", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Person" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "Email already exists", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/people/{id}": {
          "get": {
            "summary": "Get one person by id",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "Person", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Person" }
              } } },
              "404": { "description": "Not found" }
            }
          },
          "patch": {
            "summary": "Update a person",
            "description":
              "Updates one Person row. A blank/absent `role` preserves the row's current role; \
               a submitted `role` is honored only for `admin` callers (a `lawyer` caller's role \
               change is ignored) and the bootstrap Owner is pinned to `owner`. An Admin cannot \
               edit or assign Owner. Structured \
               legal-name fields preserve an omitted value and store a present-but-blank one as \
               null. Authorization: the caller's `persons.role` must be `lawyer` or `admin`; \
               anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/UpdatePersonRequest" }
              } }
            },
            "responses": {
              "200": { "description": "Updated person", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Person" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No person with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "Email already exists", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          },
          "delete": {
            "summary": "Delete a person",
            "description":
              "Deletes one Person row. The configured bootstrap Owner (see \
               `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL`) is undeletable and returns 409. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin`; \
               anonymous and `client` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "Deleted person", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Person" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No person with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The bootstrap Owner cannot be deleted", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/people/{id}/welcome": {
          "post": {
            "summary": "Send this person the welcome email",
            "x-mcp-tool": "aida_send_welcome_email",
            "description":
              "Renders and dispatches the welcome email to the Person, journaling one \
               `sent_emails` row per attempt. Authorization: the caller's `persons.role` must be \
               `lawyer` or `admin`; anonymous and `client` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "Welcome email dispatched", "content": { "application/json": {
                "schema": { "type": "object", "properties": { "status": { "type": "string" } } }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No person with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "502": { "description": "The email could not be dispatched", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/entities": {
          "get": {
            "summary": "List all entities",
            "x-mcp-tool": "aida_list_entities",
            "responses": {
              "200": { "description": "Entity list", "content": { "application/json": {
                "schema": { "type": "array", "items": { "$ref": "#/components/schemas/Entity" } }
              } } }
            }
          },
          "post": {
            "summary": "Create an entity",
            "description":
              "Creates one Entity row from a name, an entity type, and a jurisdiction. \
               The name is required and trimmed; the type and jurisdiction must reference \
               existing rows. Entity names are deliberately not unique — namesakes are real — \
               with one exception: the firm's own anchor Entity (`NAVIGATOR_BOOTSTRAP_COMPANY`, \
               falling back to the shipped firm) cannot be duplicated and returns 409. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin`; \
               anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/CreateEntityRequest" }
              } }
            },
            "responses": {
              "201": { "description": "Created entity", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Entity" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed: a blank name, or an entity type or jurisdiction that references no existing row", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The firm anchor entity already exists", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/seed": {
          "post": {
            "summary": "Reconcile a seed document",
            "description": "Applies one YAML document in Navigator's standard `lookup_fields` / `records` seed format. The model is a supported singular glossary term. Existing lookup matches are unchanged by default; `overwrite` replaces the fields represented in the seed record. Authorization: Lawyer tier only.",
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/SeedRequest" }
              } }
            },
            "responses": {
              "200": { "description": "Reconciliation summary", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/SeedReport" }
              } } },
              "401": { "description": "No authenticated session" },
              "403": { "description": "Authenticated caller is not Lawyer/admin" },
              "422": { "description": "Unsupported model or invalid seed document" }
            }
          }
        },
        "/app/api/entities/{id}": {
          "get": {
            "summary": "Get one entity by id",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "Entity", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Entity" }
              } } },
              "404": { "description": "Not found" }
            }
          },
          "patch": {
            "summary": "Update an entity",
            "description":
              "Replaces the name, entity type, and jurisdiction of one Entity row. The name is \
               required and the type and jurisdiction must reference existing rows. The firm's own \
               anchor Entity (`NAVIGATOR_BOOTSTRAP_COMPANY`, falling back to the shipped firm) has \
               an immutable name — its type and jurisdiction remain editable — and renaming any \
               other Entity *into* the anchor's name is refused; both return 409. The name is \
               compared byte for byte, so a case or whitespace variant of the anchor's name counts \
               as a rename. Authorization: the caller's `persons.role` must be `lawyer` or \
               `admin`; anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/UpdateEntityRequest" }
              } }
            },
            "responses": {
              "200": { "description": "Updated entity", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Entity" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed — a blank name, or an entity type or jurisdiction naming no existing row", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No entity with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The firm anchor's name is immutable, or the rename would duplicate it", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          },
          "delete": {
            "summary": "Delete an entity",
            "description":
              "Removes one Entity row and returns it. The firm's own anchor Entity \
               (`NAVIGATOR_BOOTSTRAP_COMPANY`, falling back to the shipped firm) is undeletable and \
               returns 409 — `store::seed` re-creates that row by exact name on every boot, so \
               removing it would never stick. An Entity other rows still reference (a matter, say) \
               is also refused with 409, carrying the database's own detail naming the referencing \
               table. Authorization: the caller's `persons.role` must be `lawyer` or `admin`; \
               anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The deleted entity", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Entity" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No entity with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The firm anchor is protected, or other rows still reference this entity", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/jurisdictions": {
          "get": {
            "summary": "List all jurisdictions",
            "x-mcp-tool": "aida_list_jurisdictions",
            "responses": {
              "200": { "description": "Jurisdiction list", "content": { "application/json": {
                "schema": { "type": "array",
                            "items": { "$ref": "#/components/schemas/Jurisdiction" } }
              } } }
            }
          }
        },
        "/app/api/entity-types": {
          "get": {
            "summary": "List all entity types",
            "responses": {
              "200": { "description": "EntityType list", "content": { "application/json": {
                "schema": { "type": "array",
                            "items": { "$ref": "#/components/schemas/EntityType" } }
              } } }
            }
          }
        },
        "/app/api/project-repositories": {
          "get": {
            "summary": "Reconcile every matter against the repository it records (admin)",
            "description":
              "Reports where a Project row and its `repository_url` disagree, across every matter \
               in the deployment. A Project code is its repository name, so a row whose recorded \
               URL names a different repository is drift provable from the row alone — no checkout \
               and no configuration required. Findings carry a `severity` of `warn` or `fail`; the \
               report's `reconciled` is false when any finding is a `fail`. Where the deployment \
               has a configured forge pair, a row recorded outside it is reported as a `warn` and \
               `compared_against_deployment_forge` is true; where it has none that comparison is \
               skipped and the flag is false. Authorization: admin-tier only (`owner`/`admin`) — \
               unlike every other matter read here, this deliberately reads all rows rather than \
               the caller's participation-scoped lens, so `lawyer`, `clerk`, and `client` are \
               rejected.",
            "responses": {
              "200": { "description": "The reconciliation report", "content": { "application/json": { "schema": { "type": "object" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not admin-tier", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The matters could not be read", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects": {
          "get": {
            "summary": "List the caller's matters",
            "x-mcp-tool": "aida_list_projects",
            "description":
              "Every matter the caller may see, already scoped — the directory lens for \
               Owner/Admin, participation for lawyer/clerk, the client's own matters for a client. \
               Authorization: any authenticated session.",
            "responses": {
              "200": { "description": "The caller's visible matters", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The matters could not be read", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "post": {
            "summary": "Open a matter",
            "x-mcp-tool": "aida_create_project",
            "description":
              "Opens a new matter: it runs the conflict check, requires the opening attorney's \
               conflict attestation, designates both DRIs, and provisions the matter's repository, \
               all as one all-or-nothing operation. The client of record must be a pre-existing \
               `client`-role person (never a firm attorney), and the entity must \
               already exist. `attestation` must be `true` — a matter open with no attestation is \
               refused (`attestation_required`). A **blocking** conflict (adverse to a current \
               client) is a hard `409 conflict_blocked` that no attestation overrides. The \
               attester is the authenticated session's person, recorded on the attestation audit \
               row and designated the accountable lawyer DRI — never taken from the request body. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin`; at this firm \
               `lawyer` is an attorney, so this is the 'an attorney is opening and attesting' gate. \
               Anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/OpenMatterRequest" }
              } }
            },
            "responses": {
              "201": { "description": "The opened matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Project" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed, a bad client/entity reference, a non-client client of record, a non-lawyer attester, or a missing attestation", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "A blocking conflict (adverse to a current client) or a duplicate project code", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "502": { "description": "The matter's repository could not be provisioned (the open was rolled back)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}": {
          "get": {
            "summary": "Get one matter",
            "description":
              "One matter the caller may see. Authorization: any authenticated session; a matter \
               the caller does not participate in returns a non-disclosing 404 (Owner/Admin see \
               all).",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The matter", "content": { "application/json": { "schema": { "type": "object" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "patch": {
            "summary": "Update a matter's descriptive fields",
            "description":
              "Updates the name, entity, and scope narrative of an existing matter. This is the \
               descriptive update only: it runs no conflict check and provisions no repo (those \
               belong to the matter-open path), and it does not change the matter's \
               lifecycle `status`/`closed_at` — moving through open/closed/archived is a transition \
               with firm retention semantics, handled by dedicated lifecycle commands, not this \
               edit. `name` is required; `entity_id` and `description` are optional — an omitted \
               field is left untouched and a blank `description` clears it. Authorization: the \
               caller's `persons.role` must be `lawyer` or `admin`; anonymous, `client`, and \
               non-lawyer `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/UpdateProjectRequest" }
              } }
            },
            "responses": {
              "200": { "description": "The updated matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Project" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed — a blank name or an entity that does not exist", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No matter with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          },
          "delete": {
            "summary": "Delete a matter",
            "description":
              "Removes one matter and returns it. A matter opened the normal way carries dependent \
               rows (DRI participations, later notations and documents), \
               whose foreign keys block the delete — returned as 409 with the database's own detail \
               naming the referencing table, a conflict the caller resolves by detaching or closing \
               those records first. Only a matter with no dependents deletes cleanly. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin`; \
               anonymous, `client`, and non-lawyer `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The deleted matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Project" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No matter with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "Other records still reference this matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/close": {
          "post": {
            "summary": "Open a matter's closing-letter notation",
            "description":
              "Opens the firm-signed `closing__letter` notation on an existing matter, addressed to \
               the matter's client — the REST mirror of the lawyer close control, converging on the \
               same command. `201 Created` carrying the new notation's id; the walk that follows \
               (and the eventual flip to `closed`) is driven separately. Authorization: the \
               caller's `persons.role` must be `lawyer` or `admin`, and the caller must \
               participate in the matter — an out-of-scope matter returns 404, never disclosing its \
               existence (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "201": { "description": "The closing notation was opened", "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["notation_id"],
                  "properties": {
                    "notation_id": { "type": "string", "format": "uuid" }
                  }
                }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such matter, or the caller does not participate in it", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The matter has no client to address the closing letter to", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The closing notation could not be opened", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/approve-plan": {
          "post": {
            "summary": "Approve a released estate plan (client)",
            "description":
              "The client approves their released estate plan: fires the `client_approved` \
               transition and marks every released draft approved — the REST mirror of the client \
               approve control, converging on the same command. `204 No Content` on success. This \
               is a client-writable door: any authenticated caller reaches the policy layer, and the \
               command enforces client-lens matter access. A caller who is not this matter's client, \
               a matter with no plan awaiting the client's approval, or a session with no linked \
               Person all return a non-disclosing 404.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The estate plan was approved" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No plan awaiting this client's approval on that matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The estate plan could not be approved", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/conversation": {
          "get": {
            "summary": "Read a matter's conversation",
            "description":
              "The matter conversation thread. A client sees the client-visible thread (internal \
               notes filtered out); the firm sees the full thread. Authorization: any authenticated \
               session; out-of-scope matter → 404.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The conversation", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/projects/{id}/conversation/messages": {
          "post": {
            "summary": "Post a message to a matter's conversation",
            "description":
              "Posts one message to a matter's conversation — the REST mirror of the portal message \
               control, converging on the same command. `204 No Content` on success. This is a \
               client-writable door: any authenticated caller reaches the policy layer, and the \
               command enforces either-lens matter access (a non-participant is a non-disclosing \
               404). The caller's tier decides the side: a client's message is inbound, a lawyer's \
               is outbound, or an internal note when `internal` is set (a client's `internal` flag \
               is ignored).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["body"],
                  "properties": {
                    "body": { "type": "string", "description": "The message body" },
                    "internal": {
                      "type": "boolean",
                      "description": "Lawyer lens only: post as an internal note rather than a client-visible message"
                    }
                  }
                }
              } }
            },
            "responses": {
              "204": { "description": "The message was posted" },
              "400": { "description": "The message body is empty", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such matter, or the caller does not participate in it", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The message could not be posted", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/participants": {
          "get": {
            "summary": "List a matter's participation ledger",
            "description":
              "The participation rows on a matter the caller may see. Authorization: any \
               authenticated session; out-of-scope matter → 404.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The participation ledger", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "post": {
            "summary": "Add a person to a matter's participation ledger",
            "x-mcp-tool": "aida_link_person_project",
            "description":
              "Adds a person to a matter as a participant. The matter and the person must \
               pre-exist, and a person appears on a matter at most once. The body names only the \
               person: their matter-side participation is derived from `persons.role`, so a \
               `client` lands on the client side and a firm tier lands firm-side. This is the same \
               command the lawyer participation form funnels through. Designating the accountable \
               DRI is a separate concern. Authorization: the caller's `persons.role` must be \
               `lawyer` or `admin`; anonymous, `client`, and non-lawyer `clerk` callers are \
               rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddParticipantRequest" }
              } }
            },
            "responses": {
              "201": { "description": "The created participation row", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Participation" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No matter or no person with that id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "That person is already assigned to this matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The participation row could not be persisted (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/participants/{role_id}": {
          "patch": {
            "summary": "Edit a matter participation row",
            "description":
              "Re-points a participation row at a different person; the participation is re-derived \
               from that person's `persons.role`. The row must belong to the matter, the person \
               must exist, no other row may already place them on the matter, and the edit must \
               not strand the matter's lawyer DRI (the accountability marker rides this row, so \
               moving it to another person or to a client-tier one is refused — reassign the DRI \
               first). Same command the lawyer form uses. Authorization: the caller's \
               `persons.role` must be `lawyer` or `admin`; anonymous, `client`, and \
               non-lawyer `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "role_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/UpdateParticipantRequest" }
              } }
            },
            "responses": {
              "200": { "description": "The updated participation row", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/Participation" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such participation row on that matter, or no such person", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The person is already on the matter, or the edit would strand the lawyer DRI", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The participation row could not be updated (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          },
          "delete": {
            "summary": "Remove a matter participant",
            "description":
              "Removes a participation row from a matter. The row must belong to the matter and must \
               not be the matter's lawyer DRI — removing it would strand the accountable lawyer, so \
               reassign the DRI first. `204 No Content` on success. Authorization: the caller's \
               `persons.role` must be `lawyer` or `admin`; anonymous, `client`, and non-lawyer \
               `clerk` callers are rejected.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "role_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The participant was removed" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such participation row on that matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "That row is the matter's lawyer DRI and cannot be removed", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The participation row could not be removed (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/participants/{role_id}/dri": {
          "put": {
            "summary": "Designate a matter participant as a DRI",
            "description":
              "Marks the participation row named by `role_id` as one of the matter's directly \
               responsible individuals. The side — lawyer DRI or client DRI — follows from that \
               person's tier and is not a field the caller names, so this door takes no body. \
               `204 No Content` on success. Authorization: the caller's `persons.role` must be \
               `lawyer` or `admin`; the shared command additionally gates the change on the caller \
               already holding the marker for that side (a matter's lawyer DRIs govern their own \
               set), so a lawyer who is not a current holder is refused `403`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "role_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The participant now carries the DRI marker" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin, or does not hold the marker for that side", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such participation row on that matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "422": { "description": "That person cannot hold the DRI marker on that side", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The designation could not be written (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          },
          "delete": {
            "summary": "Clear a matter participant's DRI marker",
            "description":
              "Removes the DRI marker from the participation row named by `role_id`. `204 No \
               Content` on success; clearing a marker that is not set is an accepted no-op. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin` and, like \
               designation, the caller must already hold the marker for that side (`403` \
               otherwise). The command refuses to remove the matter's last lawyer DRI — that would \
               strand its accountable lawyer — answering `409`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "role_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The participant no longer carries the DRI marker" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin, or does not hold the marker for that side", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such participation row on that matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "422": { "description": "That row is the matter's last lawyer DRI; a matter always keeps one", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The marker could not be cleared (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/notations": {
          "get": {
            "summary": "List a matter's notations",
            "description":
              "The notations opened on a matter the caller may see. Authorization: any \
               authenticated session; out-of-scope matter → 404.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The matter's notations", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "post": {
            "summary": "Open a notation on a matter",
            "x-mcp-tool": "aida_create_notation",
            "description":
              "Opens a notation on an existing matter from a template. The template is read from the \
               matter's own git repo, auto-saved as an immutable version, and the notation opens \
               pinned to it; a code absent from the repo falls back to the bundled firm catalog. \
               The client is resolved by email (a `client`-role Person is created on first sight \
               and attached to the matter). The first notation on a matter must be the engagement \
               that opens it (a retainer or an onboarding). This is the same command the lawyer \
               notation form funnels through. Authorization: the caller's `persons.role` must be \
               `lawyer` or `admin` (anonymous, `client`, and non-lawyer `clerk` callers are \
               rejected), and the caller must additionally participate in the matter — an \
               out-of-scope matter returns 404, never disclosing its existence (`admin` bypasses \
               the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/CreateNotationRequest" }
              } }
            },
            "responses": {
              "201": { "description": "The opened notation's id", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/CreateNotationResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed — template_code or client_email is blank", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such matter (or it is outside the caller's scope), or no such template", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The client email already belongs to another person", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "422": { "description": "The template is not the required engagement, or it fails a blocking authoring rule", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The notation could not be opened (repo unconfigured or database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}": {
          "get": {
            "summary": "Get one notation",
            "description":
              "One notation, scoped by its matter. Authorization: any authenticated session; a \
               notation whose matter the caller does not participate in returns a non-disclosing \
               404 (Owner/Admin see all).",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The notation", "content": { "application/json": { "schema": { "type": "object" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such notation, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/notations/{id}/review-documents": {
          "get": {
            "summary": "List a notation's review drafts",
            "description":
              "The review drafts rendered on a notation (firm work product). Authorization: lawyer \
               or admin, and the caller must participate in the notation's matter (out-of-scope → \
               404).",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The review drafts", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such notation, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/notations/{id}/answers": {
          "post": {
            "summary": "Answer a notation's current questionnaire step",
            "x-mcp-tool": "aida_answer_notation",
            "description":
              "Records an answer to the step the notation's questionnaire is currently asking, \
               attributed to the acting lawyer (the notation's bound Person stays the respondent). \
               The questionnaire advances one step at a time: `question_code` must match the step \
               being asked — an out-of-order answer is rejected with `409 question_mismatch` \
               carrying the expected code. Returns where the questionnaire is next: another \
               question to collect, or `complete`. This is the machine/lawyer surface; the \
               client-facing self-serve intake (the magic-link walk) is a separate flow, not this \
               REST path. Authorization: the caller's `persons.role` must be `lawyer` or \
               `admin` (anonymous, `client`, and non-lawyer `clerk` callers are rejected), and the \
               caller must additionally participate in the notation's matter — an out-of-scope or \
               unknown notation returns 404, never disclosing it (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AnswerStepRequest" }
              } }
            },
            "responses": {
              "200": { "description": "The questionnaire's next step (a question, or complete)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/NotationStep" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The answer names a different step than the one being asked, or the questionnaire is already complete", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "422": { "description": "The question is not answerable through this door (not client-facing, or not flagged for re-collection)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The answer could not be recorded (runtime or database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/request-changes": {
          "post": {
            "summary": "Send a notation back to its client for changes",
            "description":
              "Sends a notation parked at `lawyer_review` back to its client for changes: records the \
               flagged answer codes (and an optional note) and fires the `changes_requested` \
               transition — the REST mirror of the lawyer request-changes control, converging on the \
               same command. `204 No Content` on success. Authorization: the caller's `persons.role` \
               must be `lawyer` or `admin`, and the caller must participate in the notation's matter \
               — an out-of-scope or unknown notation returns 404 (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["flagged"],
                  "properties": {
                    "flagged": {
                      "type": "array",
                      "items": { "type": "string" },
                      "description": "The question codes to send back for re-collection"
                    },
                    "note": { "type": "string", "description": "An optional note to the client" }
                  }
                }
              } }
            },
            "responses": {
              "204": { "description": "The notation was sent back for changes" },
              "400": { "description": "No answer was flagged", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or the caller does not participate in its matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The notation is not awaiting review", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The change request could not be recorded", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/reask": {
          "post": {
            "summary": "Resubmit a re-collected notation for review",
            "description":
              "Resubmits a notation parked at re-collection: every flagged question must carry a \
               non-empty re-collected value in `answers` (keyed by bare question code), each is \
               written and the `intake_resubmitted` transition fires — the REST mirror of the lawyer \
               reask control, converging on the same command. `204 No Content` on success. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin`, and the \
               caller must participate in the notation's matter — an out-of-scope or unknown \
               notation returns 404 (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["answers"],
                  "properties": {
                    "answers": {
                      "type": "object",
                      "additionalProperties": { "type": "string" },
                      "description": "Re-collected values keyed by bare question code"
                    }
                  }
                }
              } }
            },
            "responses": {
              "204": { "description": "The notation was resubmitted for review" },
              "400": { "description": "A flagged answer was left blank", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or the caller does not participate in its matter", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The notation is not awaiting re-collection", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The re-collection could not be resubmitted", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/intake": {
          "post": {
            "summary": "Email a notation's client their self-serve intake link",
            "description":
              "Sends the notation's bound client the secure magic link that backs their self-serve \
               intake walk, ensuring the client participates in the matter first. Email delivery is \
               best-effort — a send failure is logged, not surfaced, since the link is idempotent \
               and can be re-sent — so `200` reports the link was dispatched to the returned \
               recipient. Same command the lawyer form drives. Authorization: the caller's \
               `persons.role` must be `lawyer` or `admin` (anonymous, `client`, and non-lawyer \
               `clerk` callers are rejected), and the caller must additionally participate in the \
               notation's matter — an out-of-scope or unknown notation returns 404, never \
               disclosing it (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The intake link was dispatched", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/SendIntakeResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The notation has no client to send an intake link to", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The intake could not be dispatched (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/approval": {
          "post": {
            "summary": "Approve a notation parked at lawyer_review",
            "description":
              "The attorney approves a notation parked at `lawyer_review`: re-assembles the reviewed \
               document (template + custom clauses, or a filled government AcroForm) and fires the \
               `approved` transition so the worker renders and persists its PDF, parking at the \
               generate-PDF step. The deliberate signature dispatch is a separate door. Idempotent: \
               if the PDF is already rendered (a prior approve, or a clean machine-only intake that \
               walked straight through), approving again is a no-op that reports the current state. \
               Returns the notation's workflow state after the action. Same command the lawyer \
               review screen drives. Authorization: the caller's `persons.role` must be lawyer \
               `lawyer` or `admin` (anonymous, `client`, and non-lawyer `clerk` callers are \
               rejected), and the caller must additionally participate in the notation's matter — \
               an out-of-scope or unknown notation returns 404 (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The notation's workflow state after approval", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/NotationLifecycleResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "422": { "description": "A government form could not be prepared (missing blank, pin mismatch, or mis-mapped field)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The approval could not be recorded (runtime, storage, or database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/signature": {
          "post": {
            "summary": "Dispatch a notation's document for signature",
            "description":
              "Sends the notation's rendered document out for signature: fires `pdf_persisted` and \
               dispatches exactly one envelope through the signature provider (the client signs \
               first, the firm countersigns). Requires the document PDF to have been rendered by a \
               prior approval — if the worker has not persisted it yet, returns `409 \
               document_not_ready` (retry) rather than sending a missing document. Idempotent: a \
               notation that already has an envelope out reuses its request id and sends nothing. \
               Returns the workflow state and the provider's signature request id (which the \
               inbound completion webhook correlates back to this notation). Same command the \
               lawyer form drives. Authorization: the caller's `persons.role` must be `lawyer` \
               or `admin` (anonymous, `client`, and non-lawyer `clerk` callers are rejected), and \
               the caller must additionally participate in the notation's matter — an out-of-scope \
               or unknown notation returns 404 (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The envelope was dispatched (or already out)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/NotationSignatureResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The document PDF has not been rendered yet — approve first, then retry", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The envelope could not be dispatched (provider, storage, or database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/release-drafts": {
          "post": {
            "summary": "Release an estate notation's drafts to client review",
            "description":
              "The attorney gate for an estate matter: at `lawyer_review`, advance the notation to \
               `client_review` and flip every generated draft instrument to `pending_review`, which \
               is what makes it visible to the client on the review surface. No auto-generated \
               client-facing legal document leaves `draft` without this human step. Returns the \
               notation's workflow state after the gate. Same command the lawyer form drives. \
               Authorization: the caller's `persons.role` must be `lawyer` or `admin` \
               (anonymous, `client`, and non-lawyer `clerk` callers are rejected), and the caller \
               must additionally participate in the notation's matter — an out-of-scope or unknown \
               notation returns 404 (`admin` bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The notation's workflow state after releasing drafts", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/NotationLifecycleResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The notation is not at the lawyer-review gate; drafts cannot be released", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/review-documents/{id}/comments": {
          "post": {
            "summary": "Add an anchored comment to a review document",
            "description":
              "Adds one anchored comment to a review document (a ProseMirror range plus the text it \
               covered) and folds it into the matter's privileged conversation log. This is the \
               client-writable review surface, so — unlike every other write here — any \
               authenticated caller is admitted, and the door then enforces **client-lens** matter \
               scope: the caller must participate in the document's matter through the client side \
               (the same gate the read-only review surface uses), so a firm-side-only lawyer or a \
               non-participant receives 404, and a still-draft document is never disclosed. The \
               comment's `direction` in the conversation log is derived from the caller's role (a \
               client's comment is inbound, a lawyer/admin comment outbound); a Clerk has no \
               review-comment capability and is refused.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddReviewCommentRequest" }
              } }
            },
            "responses": {
              "201": { "description": "The created comment and its conversation-log spine row", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddReviewCommentResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed — blank body or invalid anchor range", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such (client-visible, non-draft) review document, or the caller can't author here", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The comment could not be recorded (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/documents/{id}/deletion-requests": {
          "post": {
            "summary": "Request a document's deletion",
            "description":
              "A matter participant (typically the client) asks for a document to be deleted. This \
               only records a `pending` request — a lawyer/admin must later authorize the actual \
               expunge; nothing is deleted here. Client-writable like the review surface: any \
               authenticated caller is admitted, then the door enforces **client-lens** matter \
               scope (the caller must participate in the document's matter through the client \
               side), so a firm-side-only lawyer or a non-participant receives 404. Idempotent: a \
               second ask while one is already pending returns the existing request (`200`); a \
               fresh request is `201`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "201": { "description": "A fresh pending deletion request was recorded", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/DeletionRequestResponse" }
              } } },
              "200": { "description": "A pending deletion request already existed (idempotent)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/DeletionRequestResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated, but the session has no linked person to attribute the request to", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such document, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The request could not be recorded (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/clauses": {
          "post": {
            "summary": "Append a custom clause to a notation's document",
            "description":
              "Appends one firm-authored clause to a notation's document; the clause is spliced into \
               the assembled body at render time (after the template, in append order). Same store \
               command the lawyer clause form drives. Authorization: the caller's `persons.role` \
               must be `lawyer` or `admin` (anonymous, `client`, and non-lawyer `clerk` \
               callers are rejected), and the caller must additionally participate in the \
               notation's matter — an out-of-scope or unknown notation returns 404 (`admin` \
               bypasses the scope check).",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddClauseRequest" }
              } }
            },
            "responses": {
              "201": { "description": "The appended clause", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddClauseResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed — the clause body is blank", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such notation, or it is outside the caller's scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The clause could not be appended (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/clauses/{clause_id}": {
          "patch": {
            "summary": "Edit a notation clause",
            "description":
              "Replaces one clause's body. Lawyer-tier only and matter-scoped; the clause must belong \
               to the notation in the path (else 404), and a blank body is rejected (400). Same \
               store command the lawyer clause form drives.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "clause_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddClauseRequest" }
              } }
            },
            "responses": {
              "200": { "description": "The edited clause", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/AddClauseResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Validation failed — the clause body is blank", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such clause on that notation, or the notation is out of scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The clause could not be edited (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          },
          "delete": {
            "summary": "Remove a notation clause",
            "description":
              "Deletes one clause from a notation's document. Lawyer-tier only and matter-scoped; the \
               clause must belong to the notation in the path. `204 No Content` on success.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "clause_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The clause was removed" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such clause on that notation, or the notation is out of scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The clause could not be removed (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/notations/{id}/clauses/{clause_id}/move": {
          "post": {
            "summary": "Reorder a notation clause",
            "description":
              "Swaps a clause with its neighbour in render order (`direction: up|down`; a move at the \
               ends is an idempotent no-op). Lawyer-tier only and matter-scoped; the clause must \
               belong to the notation in the path. `204 No Content`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } },
              { "name": "clause_id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/MoveClauseRequest" }
              } }
            },
            "responses": {
              "204": { "description": "The clause order was updated (or unchanged at an end)" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such clause on that notation, or the notation is out of scope", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The clause could not be reordered (database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/projects/{id}/contract-review": {
          "post": {
            "summary": "Upload a contract for playbook review",
            "description":
              "Uploads an inbound third-party contract for deviation review as `multipart/form-data`: \
               either a `file` part (the contract) or a `text` part (the pasted contract). Opens a \
               `services__contract_review` notation, files the contract, runs the deviation analysis \
               against the client company's playbook, and lands the matter at `lawyer_review`. Same \
               command the lawyer/portal upload form drives. Client-writable: a matter's client may \
               submit their own contract, or the firm may — so any authenticated caller is admitted, \
               and the door enforces matter scope through either lens (lawyer or client); a \
               non-participant receives 404. If the company has no playbook on file yet, returns \
               422.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "multipart/form-data": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "file": { "type": "string", "format": "binary", "description": "The contract file (preferred over `contract_text`)." },
                    "contract_text": { "type": "string", "description": "The pasted contract text, used when no file is given." }
                  }
                }
              } }
            },
            "responses": {
              "201": { "description": "The created contract review", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ContractReviewResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "The session has no linked person to attribute the contract to", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "400": { "description": "Malformed multipart, or neither a file nor non-empty text was provided", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such project, or the caller does not participate in it", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "422": { "description": "The client company has no contract-review playbook on file", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The review could not be run (runtime, reviewer, or database error)", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/contract-reviews/{id}": {
          "get": {
            "summary": "Get one inbound-contract review",
            "description":
              "One inbound-contract review, scoped to its matter. Authorization: lawyer or admin, \
               and the caller must participate in the review's matter (out-of-scope → 404).",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The contract review", "content": { "application/json": { "schema": { "type": "object" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such review, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/contract-reviews/{id}/findings/{idx}": {
          "post": {
            "summary": "Save an attorney's decision on a review finding",
            "description":
              "Saves the attorney's edits and accept/reject decision on one finding of an inbound \
               contract review, recording the decision to the immutable audit trail — the REST \
               mirror of the review surface, converging on the same command. `204 No Content` on \
               success. Authorization: lawyer or admin, and the caller must participate in the \
               review's matter (out-of-scope → 404). A closed review is `409`; an out-of-range \
               finding index is `404`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
              { "name": "idx", "in": "path", "required": true, "schema": { "type": "integer", "minimum": 0 } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["accept"],
                  "properties": {
                    "accept": { "type": "boolean", "description": "Accept (true) or reject (false) the finding for delivery" },
                    "severity": { "type": "string", "enum": ["low", "medium", "high"] },
                    "suggested_redline": { "type": "string" },
                    "attorney_note": { "type": "string" }
                  }
                }
              } }
            },
            "responses": {
              "204": { "description": "The decision was recorded" },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such review/finding, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "409": { "description": "The review is closed", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The decision could not be saved", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/contract-reviews/{id}/summary": {
          "post": {
            "summary": "Edit a contract review's risk summary",
            "description":
              "Edits the risk summary of an inbound contract review — the REST mirror of the review \
               surface, converging on the same command. `204 No Content` on success. Authorization: \
               lawyer or admin, and the caller must participate in the review's matter (out-of-scope \
               → 404).",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["risk_summary"],
                  "properties": { "risk_summary": { "type": "string" } }
                }
              } }
            },
            "responses": {
              "204": { "description": "The summary was saved" },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such review, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The summary could not be saved", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/contract-reviews/{id}/approve": {
          "post": {
            "summary": "Approve a contract review and deliver the memo",
            "description":
              "Assembles the review memo from the signed-off findings and risk summary, files it \
               into the matter, and drives the workflow to completion — the REST mirror of the \
               review surface, converging on the same command. `204 No Content` on success. \
               Authorization: lawyer or admin, and the caller must participate in the matter. Not at \
               the review gate is `409`; approving before every finding has an accept/reject \
               decision is `422`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The memo was delivered and the review approved" },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such review, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "409": { "description": "The review is not at the approval gate", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "422": { "description": "Not every finding has been acted on", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The memo could not be delivered", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/contract-reviews/{id}/reject": {
          "post": {
            "summary": "Reject a contract review without a memo",
            "description":
              "Rejects an inbound contract review without producing a memo — the REST mirror of the \
               review surface, converging on the same command. `204 No Content` on success. \
               Authorization: lawyer or admin, and the caller must participate in the matter. Not at \
               the review gate is `409`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The review was rejected" },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such review, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "409": { "description": "The review is not at the reject gate", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The review could not be rejected", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/projects/{id}/documents": {
          "get": {
            "summary": "List a matter's documents",
            "description":
              "The documents filed on a matter. A client sees only client-visible documents \
               (internal work product is filtered out); the firm sees them all. Authorization: any \
               authenticated session; out-of-scope matter → 404.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The matter's documents", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "post": {
            "summary": "File a document into a matter",
            "description":
              "Files a document into a matter — the REST mirror of the lawyer upload control, \
               converging on the same command. The browser uploads via multipart; this door takes \
               the bytes base64-encoded. `201 Created` with the new document id. Authorization: \
               lawyer or admin, and the caller must participate in the matter (out-of-scope → 404). \
               A blank filename or undecodable base64 is `400`. `visibility` defaults to internal \
               work product; pass `\"client\"` to make it client-visible.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["filename", "content_base64"],
                  "properties": {
                    "filename": { "type": "string" },
                    "content_base64": { "type": "string", "description": "Base64-encoded file bytes" },
                    "content_type": { "type": "string" },
                    "kind": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["client", "internal"] },
                    "description": { "type": "string" }
                  }
                }
              } }
            },
            "responses": {
              "201": { "description": "The document was filed", "content": { "application/json": {
                "schema": { "type": "object", "required": ["document_id"], "properties": { "document_id": { "type": "string", "format": "uuid" } } }
              } } },
              "400": { "description": "Blank filename or undecodable base64", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The document could not be filed", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/notations/{id}/transcript": {
          "post": {
            "summary": "Run a transcript against a notation's questionnaire",
            "description":
              "Runs a batch transcript against the notation's bound questionnaire, recording every \
               likely-answered inquiry as a proposed default — the REST mirror of the lawyer/CLI \
               transcript control, converging on the same command. `200 OK` with `{template_code, \
               covered, uncovered}`. Authorization: lawyer or admin, and the caller must participate \
               in the matter (out-of-scope → 404). An empty transcript is `400`; a template with no \
               questionnaire is `422`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "type": "object", "required": ["transcript"], "properties": { "transcript": { "type": "string" } } }
              } }
            },
            "responses": {
              "200": { "description": "Coverage computed", "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "properties": {
                    "template_code": { "type": "string" },
                    "covered": { "type": "array", "items": { "type": "object" } },
                    "uncovered": { "type": "array", "items": { "type": "string" } }
                  }
                }
              } } },
              "400": { "description": "Empty transcript", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such notation, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "422": { "description": "The template has no questionnaire", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The coverage pass failed", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/projects/{id}/notations/{nid}/transcript": {
          "post": {
            "summary": "File an estate matter's sitting transcript",
            "description":
              "Files a recorded sitting's transcript into an estate matter and drives the estate \
               pipeline (extract → drafts → lawyer review) — the REST mirror of the estate \
               transcript upload, converging on the same command. The browser uploads a file; this \
               door takes the transcript as text. `204 No Content` on success. Authorization: lawyer \
               or admin, and the caller must participate in the matter; the notation must belong to \
               the matter (out-of-scope or mismatched → 404). An empty transcript is `400`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } },
              { "name": "nid", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "type": "object", "required": ["transcript_text"], "properties": { "transcript_text": { "type": "string" } } }
              } }
            },
            "responses": {
              "204": { "description": "The transcript was filed and the pipeline driven" },
              "400": { "description": "Empty transcript", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such matter/notation, mismatched, or out of scope", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The transcript could not be filed", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/playbooks": {
          "get": {
            "summary": "List contract-review playbooks",
            "description": "Every firm contract-review playbook. Authorization: lawyer or admin.",
            "responses": {
              "200": { "description": "The playbooks", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The playbooks could not be read", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "post": {
            "summary": "Create a contract-review playbook",
            "description":
              "Creates a client Company's negotiating-position playbook — the yardstick an inbound \
               contract review measures against. The REST mirror of the lawyer playbook form, \
               converging on the same command; unlike the form's pipe-delimited textarea, this door \
               takes structured positions. `201 Created` with the new id. Authorization: lawyer or \
               admin. A blank name or empty position set is `400`; a duplicate name on that Company \
               is `409`.",
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["entity_id", "name", "positions"],
                  "properties": {
                    "entity_id": { "type": "string", "format": "uuid" },
                    "name": { "type": "string" },
                    "positions": {
                      "type": "array",
                      "items": {
                        "type": "object",
                        "properties": {
                          "topic": { "type": "string" },
                          "preferred": { "type": "string" },
                          "fallback": { "type": "string" },
                          "walkaway": { "type": "string" },
                          "severity": { "type": "string", "enum": ["low", "medium", "high"] }
                        }
                      }
                    }
                  }
                }
              } }
            },
            "responses": {
              "201": { "description": "The playbook was created", "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["playbook_id"],
                  "properties": { "playbook_id": { "type": "string", "format": "uuid" } }
                }
              } } },
              "400": { "description": "Blank name or empty position set", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "That Company already has a playbook with that name", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The playbook could not be created", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/playbooks/{id}": {
          "get": {
            "summary": "Get one playbook",
            "description": "One firm contract-review playbook. Authorization: lawyer or admin.",
            "parameters": [
              { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "200": { "description": "The playbook", "content": { "application/json": { "schema": { "type": "object" } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "404": { "description": "No such playbook", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          },
          "put": {
            "summary": "Replace a playbook's positions",
            "description":
              "Replaces a playbook's whole position set — the REST mirror of the lawyer edit form, \
               converging on the same command. `204 No Content` on success. Authorization: lawyer or \
               admin. An unknown playbook is `404`; an empty position set is `400`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": {
                  "type": "object",
                  "required": ["positions"],
                  "properties": {
                    "positions": {
                      "type": "array",
                      "items": {
                        "type": "object",
                        "properties": {
                          "topic": { "type": "string" },
                          "preferred": { "type": "string" },
                          "fallback": { "type": "string" },
                          "walkaway": { "type": "string" },
                          "severity": { "type": "string", "enum": ["low", "medium", "high"] }
                        }
                      }
                    }
                  }
                }
              } }
            },
            "responses": {
              "204": { "description": "The positions were replaced" },
              "400": { "description": "Empty position set", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such playbook", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The positions could not be saved", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/expunge-requests": {
          "get": {
            "summary": "List the pending document-deletion queue",
            "description": "The pending client document-deletion requests awaiting firm review. Authorization: lawyer or admin.",
            "responses": {
              "200": { "description": "The pending requests", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object" } } } } },
              "401": { "description": "No authenticated session", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } },
              "500": { "description": "The queue could not be read", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } } }
            }
          }
        },
        "/app/api/expunge-requests/{id}/authorize": {
          "post": {
            "summary": "Authorize a client document-deletion request (admin)",
            "description":
              "An admin authorizes a pending client expunge request: runs the governed expunge on \
               the requested document and marks the request authorized, linked to its audit record \
               — the REST mirror of the lawyer queue's authorize control, converging on the same \
               command. `204 No Content` on success. Authorization: admin-tier only \
               (`owner`/`admin`); `lawyer`, `clerk`, and `client` are rejected. A request already \
               resolved is `409`; an unknown request or a vanished document is `404`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The document was expunged and the request authorized" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not admin-tier", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such request, or its document is gone", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "409": { "description": "The request has already been resolved", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The expunge could not be authorized", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/expunge-requests/{id}/deny": {
          "post": {
            "summary": "Deny a client document-deletion request",
            "description":
              "A lawyer or admin denies a pending client expunge request without deleting anything \
               — the REST mirror of the lawyer queue's deny control, converging on the same \
               command. `204 No Content` on success. Authorization: the caller's `persons.role` \
               must be `lawyer` or `admin`; `client` and non-lawyer `clerk` are rejected. An \
               unknown or already-resolved request is `404`.",
            "parameters": [
              { "name": "id", "in": "path", "required": true,
                "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
              "204": { "description": "The request was denied" },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "404": { "description": "No such pending request", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "500": { "description": "The request could not be denied", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        },
        "/app/api/templates/validate": {
          "post": {
            "summary": "Lint a Template markdown file without saving it",
            "x-mcp-tool": "aida_validate_notation",
            "description":
              "Runs the Neon Law Navigator rule engine over the supplied markdown and returns the \
               violations. Stateless: no row is inserted and no Template is registered; nothing \
               is looked up or created, so this is the right call to lint a draft before it \
               exists anywhere. Mirrors `cli validate` rule-set selection: the default uses \
               `navigator_default_rules` (M-family markdown + N-family notation + S101 \
               line length); set `markdown_only: true` to drop the N-family and enable \
               `S102` line packing. Authorization: linting a Template is a lawyer authoring \
               activity, so the caller's `persons.role` must be `lawyer` or `admin`; anonymous \
               and `client` callers are rejected.",
            "requestBody": {
              "required": true,
              "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ValidateRequest" }
              } }
            },
            "responses": {
              "200": { "description": "Lint report", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ValidateResponse" }
              } } },
              "401": { "description": "No authenticated session", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } },
              "403": { "description": "Authenticated caller is not Lawyer/admin", "content": { "application/json": {
                "schema": { "$ref": "#/components/schemas/ApiError" }
              } } }
            }
          }
        }
      },
      "components": {
        "securitySchemes": {
          "bearerAuth": {
            "type": "http",
            "scheme": "bearer",
            "bearerFormat": "JWT",
            "description":
              "OIDC bearer token. In production the token is validated against the \
               configured IdP (Google Identity); in KIND the workspace's Rauthy \
               instance signs RS256 JWTs. A browser-initiated alternative — the \
               `navigator_session` cookie set by `/auth/login` — is documented as the \
               `sessionCookie` scheme. Authorization is then delegated to the Open \
               Embedded Rego policy: the policy compiled into Navigator lets any \
               authenticated session call the `/app/api/*` GET listings, and restricts the \
               write commands (people create/update/delete and welcome, entity create/update/delete, \
               project open/update/delete) and \
               the stateless \
               `/app/api/templates/validate` linter to `lawyer` and `admin`."
          },
          "sessionCookie": {
            "type": "apiKey",
            "in": "cookie",
            "name": "navigator_session",
            "description":
              "Opaque session cookie set by the OAuth Authorization Code + PKCE flow \
               at `/auth/login`. The same cookie gates the `/app/*` surface."
          }
        },
        "schemas": {
          "Person": {
            "type": "object",
            "required": ["id", "name", "email", "role",
                         "inserted_at", "updated_at"],
            "properties": {
              "id":                 { "type": "string", "format": "uuid" },
              "name":               { "type": "string" },
              "given_name":         { "type": ["string", "null"] },
              "family_name":        { "type": ["string", "null"] },
              "middle_name":        { "type": ["string", "null"] },
              "email":              { "type": "string", "format": "email" },
              "oidc_subject":       { "type": ["string", "null"],
                                      "description": "OIDC `sub` claim once the row is linked." },
              "role":               { "$ref": "#/components/schemas/PersonRole" },
              "title":              { "type": ["string", "null"] },
              "phone":              { "type": ["string", "null"] },
              "xero_contact_id":    { "type": ["string", "null"] },
              "profile_image_url":  { "type": ["string", "null"] },
              "inserted_at":        { "type": "string" },
              "updated_at":         { "type": "string" }
            }
          },
          "PersonRole": {
            "type": "string",
            "enum": ["owner", "admin", "lawyer", "clerk", "client"],
              "description": "System-wide authorization tier stored in `persons.role`; Lawyer is a person licensed to practice law and authorized for Navigator legal work, while Owner/Admin are organization-level operators and Clerk is a supervised non-lawyer."
          },
          "SeedRequest": {
            "type": "object",
            "required": ["model", "yaml"],
            "properties": {
              "model": { "type": "string", "description": "Supported singular glossary term, currently `person` or `entity`." },
              "yaml": { "type": "string", "description": "Seed YAML with `lookup_fields` and `records`." },
              "overwrite": { "type": "boolean", "default": false }
            }
          },
          "SeedReport": {
            "type": "object",
            "required": ["model", "created", "updated", "unchanged"],
            "properties": {
              "model": { "type": "string" },
              "created": { "type": "integer", "minimum": 0 },
              "updated": { "type": "integer", "minimum": 0 },
              "unchanged": { "type": "integer", "minimum": 0 }
            }
          },
          "CreatePersonRequest": {
            "type": "object",
            "required": ["name", "email"],
            "properties": {
              "name":        { "type": "string" },
              "email":       { "type": "string", "format": "email" },
              "role":        { "allOf": [ { "$ref": "#/components/schemas/PersonRole" } ],
                               "description": "Defaults to `client` when omitted or blank; non-empty invalid values are rejected." },
              "given_name":  { "type": ["string", "null"] },
              "family_name": { "type": ["string", "null"] },
              "middle_name": { "type": ["string", "null"] }
            },
            "example": {
              "name": "Libra Example",
              "email": "libra@example.com",
              "role": "client",
              "given_name": "Libra",
              "family_name": "Example"
            }
          },
          "UpdatePersonRequest": {
            "type": "object",
            "required": ["name", "email"],
            "properties": {
              "name":        { "type": "string" },
              "email":       { "type": "string", "format": "email" },
              "role":        { "allOf": [ { "$ref": "#/components/schemas/PersonRole" } ],
                               "description": "Blank/absent preserves the current role; honored only for Owner/Admin callers up to their own authority, and the bootstrap Owner is always `owner`." },
              "given_name":  { "type": ["string", "null"],
                               "description": "Omit to leave unchanged; send null or a blank string to clear." },
              "family_name": { "type": ["string", "null"],
                               "description": "Omit to leave unchanged; send null or a blank string to clear." },
              "middle_name": { "type": ["string", "null"],
                               "description": "Omit to leave unchanged; send null or a blank string to clear." }
            },
            "example": {
              "name": "Libra Example",
              "email": "libra@example.com",
              "role": "lawyer"
            }
          },
          "ApiError": {
            "type": "object",
            "required": ["error"],
            "properties": {
              "error":   { "type": "string" },
              "message": { "type": "string" }
            }
          },
          "Entity": {
            "type": "object",
            "required": ["id", "name", "entity_type_id", "jurisdiction_id",
                         "inserted_at", "updated_at"],
            "properties": {
              "id":              { "type": "string", "format": "uuid" },
              "name":            { "type": "string" },
              "entity_type_id":  { "type": "string", "format": "uuid" },
              "jurisdiction_id": { "type": "string", "format": "uuid" },
              "inserted_at":     { "type": "string" },
              "updated_at":      { "type": "string" }
            }
          },
          "Project": {
            "type": "object",
            "required": ["id", "code", "name", "status", "entity_id",
                         "inserted_at", "updated_at"],
            "properties": {
              "id":                 { "type": "string", "format": "uuid" },
              "code":               { "type": "string",
                                      "description": "Stable slug, e.g. `acme-llc-formation`." },
              "name":               { "type": "string" },
              "status":             { "type": "string",
                                      "description": "`open`, `closed`, or `archived`." },
              "entity_id":          { "type": "string", "format": "uuid" },
              "description":        { "type": ["string", "null"] },
              "git_initialized_at": { "type": ["string", "null"] },
              "closed_at":          { "type": ["string", "null"] },
              "inserted_at":        { "type": "string" },
              "updated_at":         { "type": "string" }
            }
          },
          "OpenMatterRequest": {
            "type": "object",
            "required": ["name", "code", "client_id", "entity_id", "attestation"],
            "properties": {
              "name":         { "type": "string", "description": "The matter's display name." },
              "code":         { "type": "string",
                                "description": "The matter code — lowercase letters, digits, and single hyphens, starting and ending with a letter or digit, at most 80 characters. Required and never derived: it names the matter's git repository and its folder in the firm's shared drive, and the two must match exactly." },
              "client_id":    { "type": "string", "format": "uuid",
                                "description": "The client of record — a pre-existing `client`-role person, never a firm attorney." },
              "entity_id":    { "type": "string", "format": "uuid",
                                "description": "The pre-existing entity the matter opens against." },
              "description":  { "type": ["string", "null"], "description": "The matter's scope narrative." },
              "attestation":  { "type": "boolean",
                                "description": "The opening attorney's conflict attestation. Must be true; a missing attestation is refused. The attester is the authenticated session's person — never taken from this body." }
            },
            "example": {
              "name": "Acme LLC — Formation",
              "code": "acme-llc-formation",
              "client_id": "0199a1f0-0000-7000-8000-000000000001",
              "entity_id": "0199a1f0-0000-7000-8000-000000000002",
              "description": "Delaware LLC formation.",
              "attestation": true
            }
          },
          "UpdateProjectRequest": {
            "type": "object",
            "required": ["name"],
            "properties": {
              "name":        { "type": "string",
                               "description": "Full replacement; must not be blank." },
              "entity_id":   { "type": "string", "format": "uuid",
                               "description": "Omit the field entirely to leave the matter's entity unchanged; a value moves it. \
                                               `projects.entity_id` is NOT NULL, so there is no clear operation — a JSON `null` \
                                               is accepted but treated as omission (leaves the entity unchanged), never as a clear." },
              "description": { "type": "string",
                               "description": "Omit the field entirely to leave unchanged; a blank string clears it. A JSON `null` \
                                               is accepted but treated as omission (leaves the description unchanged); send \"\" to clear." }
            },
            "example": {
              "name": "Acme LLC — Formation",
              "description": "Delaware LLC formation with an operating agreement."
            }
          },
          "AddParticipantRequest": {
            "type": "object",
            "required": ["person_id"],
            "properties": {
              "person_id":     { "type": "string", "format": "uuid",
                                 "description": "An existing person to add to the matter. Their participation is derived from `persons.role` and is not an input." }
            },
            "example": {
              "person_id": "0199a1f0-0000-7000-8000-000000000003"
            }
          },
          "UpdateParticipantRequest": {
            "type": "object",
            "required": ["person_id"],
            "properties": {
              "person_id":     { "type": "string", "format": "uuid",
                                 "description": "The person the row should name (may differ from the current one). The participation is re-derived from that person's `persons.role`." }
            },
            "example": {
              "person_id": "0199a1f0-0000-7000-8000-000000000003"
            }
          },
          "Participation": {
            "type": "object",
            "required": ["id", "person_id", "project_id", "participation",
                         "inserted_at", "updated_at"],
            "properties": {
              "id":            { "type": "string", "format": "uuid" },
              "person_id":     { "type": "string", "format": "uuid" },
              "project_id":    { "type": "string", "format": "uuid" },
              "participation": { "type": "string" },
              "inserted_at":   { "type": "string" },
              "updated_at":    { "type": "string" }
            }
          },
          "CreateNotationRequest": {
            "type": "object",
            "required": ["template_code", "client_email"],
            "properties": {
              "template_code": { "type": "string",
                                 "description": "The template code to open. Read from the matter's git repo first, then the bundled firm catalog. Trimmed; must not be blank." },
              "client_email":  { "type": "string", "format": "email",
                                 "description": "The client the notation is bound to. Resolved by email; a `client`-role Person is created on first sight and attached to the matter. Trimmed; must not be blank." }
            },
            "example": {
              "template_code": "retainer",
              "client_email": "client@example.com"
            }
          },
          "CreateNotationResponse": {
            "type": "object",
            "required": ["notation_id"],
            "properties": {
              "notation_id": { "type": "string", "format": "uuid",
                               "description": "The opened notation. Drive its questionnaire next." }
            }
          },
          "AnswerStepRequest": {
            "type": "object",
            "required": ["question_code", "value"],
            "properties": {
              "question_code": { "type": "string",
                                 "description": "The step being answered. Must match the code the questionnaire is currently asking (trimmed); an out-of-order code is rejected with 409." },
              "value":         { "type": "string",
                                 "description": "The answer value, verbatim (not trimmed — a value may carry meaningful whitespace)." },
              "reference_id":  { "type": "string", "format": "uuid",
                                 "description": "For a record/reference question, the id of the picked row; `value` stays its display name. Omit for a free-typed answer." }
            },
            "example": {
              "question_code": "client_full_name",
              "value": "Ada Lovelace"
            }
          },
          "NotationStep": {
            "oneOf": [
              {
                "type": "object",
                "required": ["status", "question"],
                "properties": {
                  "status":   { "type": "string", "enum": ["needs_answer"] },
                  "question": { "$ref": "#/components/schemas/NotationQuestion" }
                }
              },
              {
                "type": "object",
                "required": ["status"],
                "properties": {
                  "status": { "type": "string", "enum": ["complete"],
                              "description": "The questionnaire reached END; nothing more to answer." }
                }
              }
            ],
            "description": "Where the questionnaire is after recording the answer — discriminated by `status`."
          },
          "NotationQuestion": {
            "type": "object",
            "required": ["id", "code", "prompt", "answer_type", "choices"],
            "properties": {
              "id":          { "type": "string", "format": "uuid" },
              "code":        { "type": "string",
                               "description": "POST this back as `question_code` to answer this step." },
              "prompt":      { "type": "string" },
              "answer_type": { "type": "string",
                               "description": "How to collect the value (e.g. text, choice, record)." },
              "choices":     { "type": "array", "items": { "$ref": "#/components/schemas/QuestionChoice" },
                               "description": "The allowed answers for a choice question; empty otherwise." }
            }
          },
          "QuestionChoice": {
            "type": "object",
            "required": ["value", "label"],
            "properties": {
              "value": { "type": "string", "description": "Send this as `value` when picking the choice." },
              "label": { "type": "string", "description": "Human-readable label for the choice." }
            }
          },
          "SendIntakeResponse": {
            "type": "object",
            "required": ["notation_id", "recipient"],
            "properties": {
              "notation_id": { "type": "string", "format": "uuid" },
              "recipient":   { "type": "string", "format": "email",
                               "description": "The client address the intake link was dispatched to." }
            }
          },
          "NotationLifecycleResponse": {
            "type": "object",
            "required": ["notation_id", "state"],
            "properties": {
              "notation_id": { "type": "string", "format": "uuid" },
              "state":       { "type": "string",
                               "description": "The notation's workflow state after the transition (e.g. `generate_pdf__retainer_pdf`)." }
            }
          },
          "NotationSignatureResponse": {
            "type": "object",
            "required": ["notation_id", "state", "signature_request_id"],
            "properties": {
              "notation_id":          { "type": "string", "format": "uuid" },
              "state":                { "type": "string",
                                        "description": "The notation's workflow state after dispatch (e.g. `sent_for_signature__pending`)." },
              "signature_request_id": { "type": "string",
                                        "description": "The provider's request id; the inbound completion webhook correlates it back to this notation." }
            }
          },
          "AddReviewCommentRequest": {
            "type": "object",
            "required": ["anchor_start", "anchor_end", "quoted_text", "body"],
            "properties": {
              "anchor_start": { "type": "integer",
                                "description": "ProseMirror start position of the covered range." },
              "anchor_end":   { "type": "integer",
                                "description": "ProseMirror end position; must be greater than anchor_start." },
              "quoted_text":  { "type": "string",
                                "description": "The document text the range covered (trimmed)." },
              "body":         { "type": "string",
                                "description": "The comment text. Required and non-blank (trimmed)." }
            },
            "example": {
              "anchor_start": 120,
              "anchor_end": 148,
              "quoted_text": "the indemnification clause",
              "body": "Can we cap this at fees paid?"
            }
          },
          "AddReviewCommentResponse": {
            "type": "object",
            "required": ["comment_id", "communication_id"],
            "properties": {
              "comment_id":       { "type": "string", "format": "uuid" },
              "communication_id": { "type": "string", "format": "uuid",
                                    "description": "The comment's row in the matter's unified conversation log." }
            }
          },
          "DeletionRequestResponse": {
            "type": "object",
            "required": ["request_id", "already_pending"],
            "properties": {
              "request_id":      { "type": "string", "format": "uuid",
                                   "description": "The pending expunge request." },
              "already_pending": { "type": "boolean",
                                   "description": "True when a pending request already existed (the ask was a no-op)." }
            }
          },
          "AddClauseRequest": {
            "type": "object",
            "required": ["body"],
            "properties": {
              "body": { "type": "string",
                        "description": "The clause markdown. Required and non-blank (trimmed)." }
            },
            "example": { "body": "The parties agree to binding arbitration in Clark County, Nevada." }
          },
          "AddClauseResponse": {
            "type": "object",
            "required": ["clause_id"],
            "properties": {
              "clause_id": { "type": "string", "format": "uuid" }
            }
          },
          "MoveClauseRequest": {
            "type": "object",
            "required": ["direction"],
            "properties": {
              "direction": { "type": "string", "enum": ["up", "down"],
                             "description": "`up` moves the clause earlier in render order; anything else moves it later." }
            },
            "example": { "direction": "up" }
          },
          "ContractReviewResponse": {
            "type": "object",
            "required": ["review_id"],
            "properties": {
              "review_id": { "type": "string", "format": "uuid",
                             "description": "The created contract review (deviation analysis attached; matter parked at lawyer_review)." }
            }
          },
          "CreateEntityRequest": {
            "type": "object",
            "required": ["name", "entity_type_id", "jurisdiction_id"],
            "properties": {
              "name":            { "type": "string",
                                   "description": "Trimmed on the way in; must not be blank." },
              "entity_type_id":  { "type": "string", "format": "uuid",
                                   "description": "An existing `/app/api/entity-types` row." },
              "jurisdiction_id": { "type": "string", "format": "uuid",
                                   "description": "An existing `/app/api/jurisdictions` row." }
            },
            "example": {
              "name": "Example Holdings LLC",
              "entity_type_id": "0199a1f0-0000-7000-8000-000000000001",
              "jurisdiction_id": "0199a1f0-0000-7000-8000-000000000002"
            }
          },
          "UpdateEntityRequest": {
            "type": "object",
            "required": ["name", "entity_type_id", "jurisdiction_id"],
            "properties": {
              "name":            { "type": "string",
                                   "description": "Full replacement; must not be blank. Immutable for the firm anchor row." },
              "entity_type_id":  { "type": "string", "format": "uuid",
                                   "description": "An existing `/app/api/entity-types` row." },
              "jurisdiction_id": { "type": "string", "format": "uuid",
                                   "description": "An existing `/app/api/jurisdictions` row." }
            },
            "example": {
              "name": "Example Holdings LLC",
              "entity_type_id": "0199a1f0-0000-7000-8000-000000000001",
              "jurisdiction_id": "0199a1f0-0000-7000-8000-000000000002"
            }
          },
          "Jurisdiction": {
            "type": "object",
            "required": ["id", "name", "code", "jurisdiction_type", "inserted_at", "updated_at"],
            "properties": {
              "id":          { "type": "string", "format": "uuid" },
              "name":        { "type": "string" },
              "code":        { "type": "string",
                               "description": "Short code, e.g. `NV`, `CA`, `US`." },
              "jurisdiction_type": { "type": "string",
                                     "enum": ["state", "country"],
                                     "description": "`state` (US state or DC) or `country` (federal sovereign)." },
              "inserted_at": { "type": "string" },
              "updated_at":  { "type": "string" }
            }
          },
          "EntityType": {
            "type": "object",
            "required": ["id", "name", "inserted_at", "updated_at"],
            "properties": {
              "id":          { "type": "string", "format": "uuid" },
              "name":        { "type": "string" },
              "inserted_at": { "type": "string" },
              "updated_at":  { "type": "string" }
            }
          },
          "ValidateRequest": {
            "type": "object",
            "required": ["contents"],
            "properties": {
              "contents":      { "type": "string",
                                 "description": "Raw markdown body, including any YAML frontmatter." },
              "path":          { "type": "string",
                                 "description": "Pretend filename so rules that key off the path \
                                                 (e.g. N103 snake_case) have something to read. \
                                                 Defaults to `template.md`." },
              "markdown_only": { "type": "boolean",
                                 "description": "Lint with `navigator_markdown_only_rules` instead \
                                                 of the default Neon Law Navigator notation set." }
            },
            "example": {
              "contents":
                "---\nkind: trust\ntitle: Trust\ncode: trust\nrespondent_type: entity\nconfidential: false\n\
                 questionnaire:\n  BEGIN:\n    _: END\n  END: {}\n\
                 workflow:\n  BEGIN:\n    next: lawyer_review\n  \
                 lawyer_review:\n    next: END\n  END: {}\n---\n\nBody.\n",
              "path": "trust.md"
            }
          },
          "ValidateResponse": {
            "type": "object",
            "required": ["path", "clean", "violations"],
            "properties": {
              "path":       { "type": "string" },
              "clean":      { "type": "boolean" },
              "violations": { "type": "array",
                              "items": { "$ref": "#/components/schemas/ValidationViolation" } }
            },
            "example": { "path": "trust.md", "clean": true,
                         "violations": [ { "code": "N112", "line": 9,
                           "message": "workflow step `lawyer_review` is allowed but its automation is not built yet (from state `lawyer_review`)" } ] }
          },
          "ValidationViolation": {
            "type": "object",
            "required": ["code", "line", "message"],
            "properties": {
              "code":    { "type": "string", "description": "Rule code, e.g. `S101`, `N101`." },
              "line":    { "type": "integer", "format": "int32" },
              "message": { "type": "string" }
            }
          }
        }
      }
    })
}

/// Every `/app/api/*` path key declared in [`document`]. Public so the
/// drift test in `web/tests/openapi_drift.rs` can compare it against
/// the routes registered in [`crate::api::routes`].
#[must_use]
pub fn documented_paths() -> Vec<String> {
    let doc = document();
    doc["paths"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Every operation declared in [`document`] as an uppercase
/// `(HTTP method, path)` pair — one entry per method key under each
/// path. This is the operation-level view of the document that
/// `web/tests/openapi_drift.rs` compares against
/// [`crate::api::documented_api_operations`], so the drift guard is
/// path+method, not path-only: an undocumented method on an
/// already-documented path is now caught.
#[must_use]
pub fn documented_operations() -> Vec<(String, String)> {
    let doc = document();
    let mut ops = Vec::new();
    if let Some(paths) = doc["paths"].as_object() {
        for (path, methods) in paths {
            if let Some(methods) = methods.as_object() {
                for verb in methods.keys() {
                    ops.push((verb.to_uppercase(), path.clone()));
                }
            }
        }
    }
    ops
}

/// MCP tools that legitimately carry no `x-mcp-tool` annotation, each
/// with the reason it has no `/app/api` twin. An entry here is a
/// decision; the absence of an entry is what
/// `every_tool_names_an_api_operation` refuses.
///
/// Keep this list short. The orphan guard is the ratchet that keeps a new
/// tool going through the same command layer the API route uses instead
/// of reaching into `store::` on its own — an exemption opts a tool out
/// of that pressure, so it needs a reason a reader can disagree with.
pub const TOOLS_WITHOUT_AN_API_OPERATION: &[(&str, &str)] = &[
    (
        "aida_list_tools",
        "Protocol, not capability: it enumerates the catalog itself. The \
         OpenAPI document is the API surface's own equivalent, so an \
         operation for it would be circular.",
    ),
    (
        "aida_bulk_import",
        "No route today. Bulk contact loading is agent-only; the API door \
         exposes single-record `POST /app/api/people` and nothing that \
         takes a batch.",
    ),
    (
        "aida_spawn_legal_council",
        "No route today. The council is an authoring aid that renders a \
         review inline and writes nothing, so there is no command for an \
         API operation to share.",
    ),
];

/// Every `x-mcp-tool` value in [`document`], paired with the operation
/// carrying it, as `(tool, METHOD, path)`.
#[must_use]
pub fn annotated_mcp_tools() -> Vec<(String, String, String)> {
    let doc = document();
    let mut out = Vec::new();
    if let Some(paths) = doc["paths"].as_object() {
        for (path, methods) in paths {
            if let Some(methods) = methods.as_object() {
                for (verb, op) in methods {
                    if let Some(tool) = op.get("x-mcp-tool").and_then(|v| v.as_str()) {
                        out.push((tool.to_string(), verb.to_uppercase(), path.clone()));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        base_url_for, document, document_with_base, documented_operations, documented_paths,
    };

    #[test]
    fn documented_operations_expands_paths_into_method_pairs() {
        let ops = documented_operations();
        // Every documented path/method pair is present, uppercased.
        assert!(ops.contains(&("GET".to_string(), "/app/api/people".to_string())));
        assert!(ops.contains(&("POST".to_string(), "/app/api/people".to_string())));
        assert!(ops.contains(&("PATCH".to_string(), "/app/api/people/{id}".to_string())));
        assert!(ops.contains(&("DELETE".to_string(), "/app/api/people/{id}".to_string())));
        assert!(ops.contains(&("POST".to_string(), "/app/api/entities".to_string())));
        assert!(ops.contains(&("PATCH".to_string(), "/app/api/entities/{id}".to_string())));
        assert!(ops.contains(&("DELETE".to_string(), "/app/api/entities/{id}".to_string())));
        assert!(ops.contains(&("PATCH".to_string(), "/app/api/projects/{id}".to_string())));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/projects/{id}/participants".to_string()
        )));
        assert!(ops.contains(&(
            "PATCH".to_string(),
            "/app/api/projects/{id}/participants/{role_id}".to_string()
        )));
        assert!(ops.contains(&(
            "DELETE".to_string(),
            "/app/api/projects/{id}/participants/{role_id}".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/projects/{id}/notations".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/answers".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/intake".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/approval".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/signature".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/review-documents/{id}/comments".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/documents/{id}/deletion-requests".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/clauses".to_string()
        )));
        assert!(ops.contains(&(
            "PATCH".to_string(),
            "/app/api/notations/{id}/clauses/{clause_id}".to_string()
        )));
        assert!(ops.contains(&(
            "DELETE".to_string(),
            "/app/api/notations/{id}/clauses/{clause_id}".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/clauses/{clause_id}/move".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/projects/{id}/contract-review".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/notations/{id}/release-drafts".to_string()
        )));
        assert!(ops.contains(&(
            "POST".to_string(),
            "/app/api/templates/validate".to_string()
        )));
        // A GET-only path expands to exactly one operation.
        let jurisdiction_ops: Vec<_> = ops
            .iter()
            .filter(|(_, p)| p == "/app/api/jurisdictions")
            .collect();
        assert_eq!(
            jurisdiction_ops,
            vec![&("GET".to_string(), "/app/api/jurisdictions".to_string())]
        );
        // Methods are always uppercase (the drift guard compares to
        // `api::documented_api_operations`, which is uppercase).
        assert!(ops
            .iter()
            .all(|(m, _)| m.chars().all(|c| c.is_ascii_uppercase())));
    }

    #[test]
    fn base_url_derives_https_from_request_host() {
        // No NAV_BASE_URL set in the test process: a real request host
        // drives the scheme + servers URL, so prod surfaces its own
        // domain without any hard-coded value in source.
        assert_eq!(
            base_url_for(Some("www.neonlaw.com")),
            "https://www.neonlaw.com"
        );
        assert_eq!(
            base_url_for(Some("localhost:8080")),
            "http://localhost:8080"
        );
    }

    #[test]
    fn base_url_falls_back_to_placeholder_without_host() {
        assert_eq!(base_url_for(None), super::PLACEHOLDER_BASE_URL);
    }

    #[test]
    fn document_with_base_threads_host_into_servers_and_contact() {
        let d = document_with_base("https://www.neonlaw.com");
        assert_eq!(d["servers"][0]["url"], "https://www.neonlaw.com");
        assert_eq!(
            d["info"]["contact"]["url"],
            "https://www.neonlaw.com/contact"
        );
    }

    #[test]
    fn document_has_openapi_version_and_paths() {
        let d = document();
        assert_eq!(d["openapi"], "3.1.0");
        assert!(d["paths"]["/app/api/people"].is_object());
        assert!(d["paths"]["/app/api/people/{id}"].is_object());
        assert!(d["paths"]["/app/api/entities"].is_object());
        assert!(d["paths"]["/app/api/jurisdictions"].is_object());
        assert!(d["paths"]["/app/api/entity-types"].is_object());
        assert!(d["paths"]["/app/api/templates/validate"]["post"].is_object());
    }

    #[test]
    fn document_declares_each_schema() {
        let d = document();
        let schemas = &d["components"]["schemas"];
        for name in [
            "Person",
            "PersonRole",
            "CreatePersonRequest",
            "ApiError",
            "Entity",
            "CreateEntityRequest",
            "UpdateEntityRequest",
            "Project",
            "OpenMatterRequest",
            "UpdateProjectRequest",
            "AddParticipantRequest",
            "UpdateParticipantRequest",
            "Participation",
            "CreateNotationRequest",
            "CreateNotationResponse",
            "AnswerStepRequest",
            "NotationStep",
            "NotationQuestion",
            "QuestionChoice",
            "SendIntakeResponse",
            "NotationLifecycleResponse",
            "NotationSignatureResponse",
            "AddReviewCommentRequest",
            "AddReviewCommentResponse",
            "DeletionRequestResponse",
            "AddClauseRequest",
            "AddClauseResponse",
            "MoveClauseRequest",
            "ContractReviewResponse",
            "Jurisdiction",
            "EntityType",
            "ValidateRequest",
            "ValidateResponse",
            "ValidationViolation",
        ] {
            assert!(schemas[name].is_object(), "missing schema {name}");
        }
    }

    #[test]
    fn id_schemas_are_uuid_strings_not_int32() {
        let d = document();
        for entity in ["Person", "Entity", "Jurisdiction", "EntityType"] {
            let id = &d["components"]["schemas"][entity]["properties"]["id"];
            assert_eq!(id["type"], "string", "{entity}.id should be string");
            assert_eq!(id["format"], "uuid", "{entity}.id should be uuid");
        }
        for path in ["/app/api/people/{id}", "/app/api/entities/{id}"] {
            let params = &d["paths"][path]["get"]["parameters"];
            let id_schema = &params[0]["schema"];
            assert_eq!(id_schema["type"], "string", "{path} id should be string");
            assert_eq!(id_schema["format"], "uuid", "{path} id should be uuid");
        }
    }

    #[test]
    fn top_level_security_requires_auth() {
        let d = document();
        let sec = d["security"].as_array().expect("top-level security array");
        assert!(
            !sec.is_empty(),
            "`/app/api/*` requires OIDC; the OpenAPI doc must declare it at the top level"
        );
        let has_bearer = sec.iter().any(|req| {
            req.as_object()
                .is_some_and(|m| m.contains_key("bearerAuth"))
        });
        let has_cookie = sec.iter().any(|req| {
            req.as_object()
                .is_some_and(|m| m.contains_key("sessionCookie"))
        });
        assert!(
            has_bearer,
            "bearerAuth must be one of the documented schemes"
        );
        assert!(
            has_cookie,
            "sessionCookie must be one of the documented schemes"
        );
    }

    #[test]
    fn no_operation_overrides_security_to_empty() {
        let d = document();
        let paths = d["paths"].as_object().expect("paths object");
        for (path, methods) in paths {
            for (verb, op) in methods.as_object().expect("methods object") {
                let sec = &op["security"];
                assert!(
                    sec.is_null(),
                    "{verb} {path} must inherit the top-level `security` requirement \
                     (no per-op override); got {sec}"
                );
            }
        }
    }

    #[test]
    fn mutating_api_operations_document_authz_failures() {
        let d = document();
        let paths = d["paths"].as_object().expect("paths object");
        for (path, methods) in paths {
            for verb in ["post", "patch", "put", "delete"] {
                let Some(op) = methods.get(verb) else {
                    continue;
                };
                let responses = op["responses"]
                    .as_object()
                    .unwrap_or_else(|| panic!("{verb} {path} must declare OpenAPI responses"));
                assert!(
                    responses.contains_key("401"),
                    "{verb} {path} must document the unauthenticated response; API writes are never anonymous"
                );
                // Every write must document how it rejects an authenticated but
                // non-authorized caller. A lawyer-tier-gated door does that with
                // 403 (wrong tier). A client-writable door (any authenticated
                // caller is admitted, then a per-matter scope check runs) does
                // it with 404 — an out-of-scope caller is told the resource does
                // not exist rather than that they lack access. Either satisfies
                // the invariant; a write documenting neither is the bug.
                assert!(
                    responses.contains_key("403") || responses.contains_key("404"),
                    "{verb} {path} must document its authenticated-but-not-authorized rejection (403 for a tier gate, or 404 for a non-disclosing scope gate)"
                );
            }
        }
    }

    #[test]
    fn bearer_and_cookie_schemes_are_declared() {
        let d = document();
        let bearer = &d["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(bearer["type"], "http");
        assert_eq!(bearer["scheme"], "bearer");
        let cookie = &d["components"]["securitySchemes"]["sessionCookie"];
        assert_eq!(cookie["type"], "apiKey");
        assert_eq!(cookie["in"], "cookie");
        assert_eq!(cookie["name"], "navigator_session");
    }

    #[test]
    fn documented_paths_matches_paths_object() {
        let d = document();
        let mut from_obj: Vec<String> = d["paths"].as_object().unwrap().keys().cloned().collect();
        from_obj.sort();
        let mut from_helper = documented_paths();
        from_helper.sort();
        assert_eq!(from_obj, from_helper);
    }

    #[test]
    fn validate_request_example_is_itself_clean() {
        // The example shipped in the OpenAPI doc is what Swagger's "Try
        // it out" pre-fills. It must lint clean under the default rule
        // set, or the first request a caller sends comes back dirty —
        // a confusing first impression of an endpoint whose whole job
        // is linting. This guards the example against rule drift.
        let d = document();
        let ex = &d["components"]["schemas"]["ValidateRequest"]["example"];
        let contents = ex["contents"].as_str().expect("example.contents string");
        let path = ex["path"].as_str().expect("example.path string");
        let file = rules::SourceFile {
            path: std::path::PathBuf::from(path),
            contents: contents.to_string(),
        };
        let error_codes: Vec<&str> = rules::navigator_default_rules()
            .iter()
            .flat_map(|r| r.lint(&file))
            .filter(|v| rules::severity_for_code(v.code) == rules::Severity::Error)
            .map(|v| v.code)
            .collect();
        // The example must carry no *blocking* errors. Its mandatory
        // lawyer_review gate earns the yellow N112 advisory, which is
        // expected and non-blocking.
        assert!(
            error_codes.is_empty(),
            "OpenAPI ValidateRequest example must lint free of errors; got {error_codes:?}"
        );
    }

    #[test]
    fn mcp_is_intentionally_absent() {
        let d = document();
        assert!(
            d["paths"]["/mcp"].is_null(),
            "/mcp is JSON-RPC and out of scope for this OpenAPI doc"
        );
    }

    /// Direction 1: every `x-mcp-tool` names a real tool. Catches a typo
    /// or a reference left behind by a renamed tool.
    #[test]
    fn every_x_mcp_tool_names_a_real_tool() {
        let catalog: Vec<String> = mcp::tools::list_tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect();
        let annotated = super::annotated_mcp_tools();
        assert!(!annotated.is_empty(), "the annotations went missing");
        for (tool, verb, path) in &annotated {
            assert!(
                catalog.contains(tool),
                "`{verb} {path}` names `{tool}`, which is not in mcp::tools::list_tools(); \
                 got catalog {catalog:?}"
            );
        }
    }

    /// Direction 2, and the point of the pair: every tool in the catalog
    /// is named by at least one operation, or is exempt with a reason.
    ///
    /// This is the ratchet. A new tool cannot merge without either naming
    /// its API operation — which is the pressure that keeps it going
    /// through the same command layer rather than reaching into `store::`
    /// on its own — or being written down here as a deliberate exception.
    #[test]
    fn every_tool_names_an_api_operation() {
        let annotated: Vec<String> = super::annotated_mcp_tools()
            .into_iter()
            .map(|(tool, _, _)| tool)
            .collect();
        let exempt: Vec<&str> = super::TOOLS_WITHOUT_AN_API_OPERATION
            .iter()
            .map(|(name, _)| *name)
            .collect();

        for descriptor in mcp::tools::list_tools() {
            let name = descriptor["name"].as_str().unwrap();
            assert!(
                annotated.iter().any(|t| t == name) || exempt.contains(&name),
                "`{name}` is in the MCP catalog but no OpenAPI operation carries \
                 `x-mcp-tool: {name}`. Annotate the operation it shares a command with, \
                 or add it to TOOLS_WITHOUT_AN_API_OPERATION with the reason it has none."
            );
        }
    }

    /// An exemption must be a decision, not an omission: it has to name a
    /// real tool and carry a written reason.
    #[test]
    fn every_exemption_names_a_real_tool_and_gives_a_reason() {
        let catalog: Vec<String> = mcp::tools::list_tools()
            .iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect();
        let annotated: Vec<String> = super::annotated_mcp_tools()
            .into_iter()
            .map(|(tool, _, _)| tool)
            .collect();

        for (name, reason) in super::TOOLS_WITHOUT_AN_API_OPERATION {
            assert!(
                catalog.contains(&(*name).to_string()),
                "`{name}` is exempted but is not a tool in the catalog"
            );
            assert!(
                reason.len() > 30,
                "`{name}`'s exemption reason is too thin to argue with: `{reason}`"
            );
            assert!(
                !annotated.iter().any(|t| t == name),
                "`{name}` is both exempted and annotated — drop the exemption"
            );
        }
    }

    /// The annotation is on the operation, so a tool inherits that route's
    /// documented authorization failures. Spot-check the shape holds for a
    /// mutating twin rather than trusting the placement.
    #[test]
    fn an_annotated_mutating_operation_still_documents_its_authz_failures() {
        let doc = document();
        let op = &doc["paths"]["/app/api/people/{id}/welcome"]["post"];
        assert_eq!(op["x-mcp-tool"], "aida_send_welcome_email");
        assert!(
            op["responses"]["403"].is_object() || op["responses"]["401"].is_object(),
            "expected a documented authz failure on the annotated operation: {op}"
        );
    }
}
