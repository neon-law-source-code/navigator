# Unit tests for the Neon Law Navigator authorization policy.
#
# These pin every decision the firm-wide access model promises — the
# allow rules AND the deny cases — directly against the production Rego
# source. `cli/tests/regorus_policy.rs` compiles both files with Regorus, so
# there is exactly one copy of the policy and nothing to drift.
#
# Run locally: cargo test -p cli --test regorus_policy

package navigator.authz_test

import rego.v1

import data.navigator.authz

# ---------- sessions ----------
# `session` is null when unauthenticated; otherwise it carries the
# singular `role` field the policy evaluates (post `roles[] → role` collapse).

owner_session := {"sub": "x", "email": "o@neonlaw.com", "exp": 9999999999, "role": "owner", "csrf_token": ""}

admin_session := {"sub": "x", "email": "a@neonlaw.com", "exp": 9999999999, "role": "admin", "csrf_token": ""}

lawyer_session := {"sub": "x", "email": "s@neonlaw.com", "exp": 9999999999, "role": "lawyer", "csrf_token": ""}

clerk_session := {"sub": "x", "email": "clerk@neonlaw.com", "exp": 9999999999, "role": "clerk", "csrf_token": ""}

client_session := {"sub": "x", "email": "c@example.com", "exp": 9999999999, "role": "client", "csrf_token": ""}

# ---------- Owner/Admin bypass ----------

# The bypass rule grants admin every path no other rule allows — pin it
# with a path that has no explicit allow rule of its own.
test_admin_bypass_reaches_unrouted_path if {
	authz.allow with input as {"path": ["not-a-mounted-surface"], "method": "GET", "session": admin_session}
}

test_owner_bypass_reaches_unrouted_path if {
	authz.allow with input as {"path": ["not-a-mounted-surface"], "method": "GET", "session": owner_session}
}

# ---------- /app/projects/* (any authenticated caller; ENG-81) ----------
# The policy admits every tier onto the one matter surface. Row scoping is the
# handler's job, so these tests pin admission only — the denial that matters
# (an Owner with no participation row) is asserted in `store::access` and in
# `server/tests/routes.rs`, where the ledger is readable.

test_client_reaches_projects if {
	authz.allow with input as {"path": ["app", "projects"], "method": "GET", "session": client_session}
}

test_lawyer_reaches_projects if {
	authz.allow with input as {"path": ["app", "projects"], "method": "GET", "session": lawyer_session}
}

test_clerk_reaches_projects if {
	authz.allow with input as {"path": ["app", "projects"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_on_projects if {
	not authz.allow with input as {"path": ["app", "projects"], "method": "GET", "session": null}
}

test_client_can_approve_plan if {
	authz.allow with input as {"path": ["app", "projects", "p1", "approve-plan"], "method": "POST", "session": client_session}
}

# A Project's client portal at `/app/projects/{code}/portal`. The rule matches
# on the first two path elements, so a four-element path relies on that prefix
# rather than on a rule of its own — pinned here because the portal mount is the
# first surface at that depth, and a policy that silently denied it would make
# the route unreachable in production while passing every passthrough-policy
# test in the workspace.
#
# Admission only, and deliberately so: the portal is *participation*-scoped, and
# the ledger this policy cannot read is what the handler consults. A client who
# is not on this matter is admitted here and answered 404 there.
test_client_reaches_a_project_portal if {
	authz.allow with input as {"path": ["app", "projects", "p1", "portal"], "method": "GET", "session": client_session}
}

test_lawyer_reaches_a_project_portal if {
	authz.allow with input as {"path": ["app", "projects", "p1", "portal"], "method": "GET", "session": lawyer_session}
}

test_anonymous_denied_on_a_project_portal if {
	not authz.allow with input as {"path": ["app", "projects", "p1", "portal"], "method": "GET", "session": null}
}

test_client_can_submit_contract_review if {
	authz.allow with input as {"path": ["app", "projects", "p1", "contract-review"], "method": "POST", "session": client_session}
}

# The old prefixes are gone, not redirected.
test_old_portal_projects_path_is_not_allowed if {
	not authz.allow with input as {"path": ["portal", "projects"], "method": "GET", "session": client_session}
}

test_old_lawyer_prefix_is_not_allowed if {
	not authz.allow with input as {"path": ["lawyer"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["lawyer", "notations"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["lawyer", "projects"], "method": "GET", "session": client_session}
}

# ---------- /app/forms/* (blank public forms) ----------

test_client_can_browse_forms if {
	authz.allow with input as {"path": ["app", "forms"], "method": "GET", "session": client_session}
}

test_anonymous_denied_on_forms if {
	not authz.allow with input as {"path": ["app", "forms"], "method": "GET", "session": null}
}

# ---------- /app/notations/* (client-lens notation documents) ----------

test_client_reaches_app_notation_document if {
	authz.allow with input as {"path": ["app", "notations", "n1", "documents", "d1"], "method": "GET", "session": client_session}
}

test_anonymous_denied_on_app_notations if {
	not authz.allow with input as {"path": ["app", "notations", "n1", "documents", "d1"], "method": "GET", "session": null}
}

# ---------- /app/lawyer/* (lawyer tier only) ----------

test_lawyer_reaches_lawyer_notations if {
	authz.allow with input as {"path": ["app", "lawyer", "notations"], "method": "GET", "session": lawyer_session}
}

test_lawyer_reaches_the_app_outline_stage if {
	authz.allow with input as {"path": ["app", "outline"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_on_the_app_outline_stage if {
	not authz.allow with input as {"path": ["app", "outline"], "method": "GET", "session": clerk_session}
}

test_client_denied_on_the_app_outline_stage if {
	not authz.allow with input as {"path": ["app", "outline"], "method": "GET", "session": client_session}
}

test_anonymous_denied_on_the_app_outline_stage if {
	not authz.allow with input as {"path": ["app", "outline"], "method": "GET", "session": null}
}

test_client_reaches_a_project_notation_outline if {
	authz.allow with input as {"path": ["app", "projects", "p1", "n1", "outline"], "method": "GET", "session": client_session}
}

test_admin_reaches_lawyer_notations if {
	authz.allow with input as {"path": ["app", "lawyer", "notations"], "method": "GET", "session": admin_session}
}

test_owner_reaches_lawyer_notations if {
	authz.allow with input as {"path": ["app", "lawyer", "notations"], "method": "GET", "session": owner_session}
}

test_client_denied_on_lawyer_notations if {
	not authz.allow with input as {"path": ["app", "lawyer", "notations"], "method": "GET", "session": client_session}
}

test_clerk_denied_on_lawyer_notations if {
	not authz.allow with input as {"path": ["app", "lawyer", "notations"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_on_app_lawyer if {
	not authz.allow with input as {"path": ["app", "lawyer"], "method": "GET", "session": null}
}

# The brand-font download rides the /app/team prefix rules above rather than a
# rule of its own, so it is asserted per tier here: all four firm tiers reach
# it, and the two unauthorized audiences do not.
test_lawyer_reaches_font_download if {
	authz.allow with input as {"path": ["app", "team", "fonts", "gorp-serif.zip"], "method": "GET", "session": lawyer_session}
}

test_admin_reaches_font_download if {
	authz.allow with input as {"path": ["app", "team", "fonts", "gorp-serif.zip"], "method": "GET", "session": admin_session}
}

test_owner_reaches_font_download if {
	authz.allow with input as {"path": ["app", "team", "fonts", "gorp-serif.zip"], "method": "GET", "session": owner_session}
}

test_clerk_reaches_font_download if {
	authz.allow with input as {"path": ["app", "team", "fonts", "gorp-serif.zip"], "method": "GET", "session": clerk_session}
}

test_client_denied_on_font_download if {
	not authz.allow with input as {"path": ["app", "team", "fonts", "gorp-serif.zip"], "method": "GET", "session": client_session}
}

test_anonymous_denied_on_font_download if {
	not authz.allow with input as {"path": ["app", "team", "fonts", "gorp-serif.zip"], "method": "GET", "session": null}
}

# Clerk is denied every /app/lawyer path, including the workbench listings.
test_clerk_denied_on_every_lawyer_path if {
	not authz.allow with input as {"path": ["app", "lawyer", "notations"], "method": "GET", "session": clerk_session}
	not authz.allow with input as {"path": ["app", "lawyer", "disclosures"], "method": "GET", "session": clerk_session}
}

# ---------- /app/lawyer and /app/admin (the two dashboards) ----------

test_lawyer_reaches_the_firm_dashboard if {
	authz.allow with input as {"path": ["app", "lawyer"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_on_the_firm_dashboard if {
	not authz.allow with input as {"path": ["app", "lawyer"], "method": "GET", "session": clerk_session}
}

test_client_denied_on_the_firm_dashboard if {
	not authz.allow with input as {"path": ["app", "lawyer"], "method": "GET", "session": client_session}
}

# The admin dashboard rides the Owner/Admin route bypass and has no rule of its
# own, so Lawyer must not reach it. This is the assertion that catches
# someone "fixing" the missing rule by copying the /app/lawyer one.
test_lawyer_denied_on_the_admin_dashboard if {
	not authz.allow with input as {"path": ["app", "admin"], "method": "GET", "session": lawyer_session}
}

test_lawyer_reaches_admin_reference_listings if {
	authz.allow with input as {"path": ["app", "admin", "entities"], "method": "GET", "session": lawyer_session}
	authz.allow with input as {"path": ["app", "admin", "entity-types"], "method": "GET", "session": lawyer_session}
	authz.allow with input as {"path": ["app", "admin", "playbooks", "new"], "method": "POST", "session": lawyer_session}
	authz.allow with input as {"path": ["app", "admin", "people.csv"], "method": "GET", "session": lawyer_session}
	authz.allow with input as {"path": ["app", "admin", "schedules", "archives", "run"], "method": "POST", "session": lawyer_session}
}

test_lawyer_denied_on_admin_people_and_analytics if {
	not authz.allow with input as {"path": ["app", "admin", "people"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["app", "admin", "people", "new"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["app", "admin", "analytics"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_on_admin_reference_listings if {
	not authz.allow with input as {"path": ["app", "admin", "entities"], "method": "GET", "session": clerk_session}
}

test_client_denied_on_admin_reference_listings if {
	not authz.allow with input as {"path": ["app", "admin", "entities"], "method": "GET", "session": client_session}
}

test_clerk_denied_on_the_admin_dashboard if {
	not authz.allow with input as {"path": ["app", "admin"], "method": "GET", "session": clerk_session}
}

# The matter directory beneath the desk (ENG-221) inherits admission from the
# same bypass and likewise has no rule of its own. It is a firm-wide list of
# every matter, so the tier that must never reach it is the one a rule copied
# from /app/lawyer would admit: Lawyer.
test_the_admin_tiers_reach_the_matter_directory if {
	authz.allow with input as {"path": ["app", "admin", "projects"], "method": "GET", "session": admin_session}
	authz.allow with input as {"path": ["app", "admin", "projects"], "method": "GET", "session": owner_session}
}

test_lawyer_denied_on_the_matter_directory if {
	not authz.allow with input as {"path": ["app", "admin", "projects"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_on_the_matter_directory if {
	not authz.allow with input as {"path": ["app", "admin", "projects"], "method": "GET", "session": clerk_session}
}

test_client_denied_on_the_matter_directory if {
	not authz.allow with input as {"path": ["app", "admin", "projects"], "method": "GET", "session": client_session}
}

# ---------- /clerk/* is retired ----------
# The supervised surface is now a rendering of `/app/projects`, not a namespace.
# A Clerk reaches the matter path (asserted above); which of the five renderings
# they get is `store::access::matter_viewer`'s decision, not the policy's, so it
# is pinned in Rust rather than here.

test_the_retired_clerk_namespace_is_not_allowed if {
	not authz.allow with input as {"path": ["clerk"], "method": "GET", "session": clerk_session}
	not authz.allow with input as {"path": ["clerk", "projects", "p1"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_on_clerk if {
	not authz.allow with input as {"path": ["clerk"], "method": "GET", "session": null}
}

# ---------- the retired nonprofit reading surfaces ----------
# The mission letter, Notations, and the transparency disclosures were the one
# grant in this policy that admitted `client` to a page outside their own
# matters. Those pages are retired, so the grant is gone and every tier — the
# authenticated ones included — is denied. Kept as a guard: reintroducing a
# blanket authenticated-reads rule would widen `client` again.

test_no_tier_reads_the_retired_foundation_pages if {
	not authz.allow with input as {"path": ["mission"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["notations"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["transparency"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["transparency", "bylaws"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["foundation"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["foundation", "mission"], "method": "GET", "session": client_session}
}

test_anonymous_denied_on_the_retired_foundation_pages if {
	not authz.allow with input as {"path": ["mission"], "method": "GET", "session": null}
	not authz.allow with input as {"path": ["notations"], "method": "GET", "session": null}
	not authz.allow with input as {"path": ["transparency"], "method": "GET", "session": null}
	not authz.allow with input as {"path": ["foundation"], "method": "GET", "session": null}
}

# Workshop reads are public and bypass this policy. Keep the client case here
# as a guard against accidentally reintroducing a policy grant for the public
# catalog or its class material.
test_client_denied_on_the_workshops_catalog_and_its_classes if {
	not authz.allow with input as {"path": ["workshops"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["workshops", "use-the-navigator"], "method": "GET", "session": client_session}
}

# Lawyer and Clerk likewise do not need a policy grant for public material;
# Owner and Admin remain covered by their general bypass, which is unrelated
# to the public route's authorization.
test_firm_side_roles_do_not_need_a_workshop_policy_grant if {
	not authz.allow with input as {"path": ["workshops"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["workshops"], "method": "GET", "session": clerk_session}
	not authz.allow with input as {"path": ["workshops", "use-the-navigator"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["workshops", "use-the-navigator"], "method": "GET", "session": clerk_session}
}

# Anonymous access is also handled by the public route rather than this policy.
test_anonymous_denied_on_the_workshops_catalog if {
	not authz.allow with input as {"path": ["workshops"], "method": "GET", "session": null}
}

# The reading grant must not leak onto the application. It names its pages
# exactly, so a path that merely starts with one of them is not covered.
test_the_reading_grant_does_not_reach_the_application if {
	not authz.allow with input as {"path": ["lawyer"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["clerk"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["mcp"], "method": "POST", "session": client_session}
}

# ---------- /workshops/* certificate claim ----------
# Workshop reads are public and bypass the policy. Only the completion
# certificate POST enters this rule family.

workshop_certificate := ["workshops", "use-the-navigator", "certificate"]

test_firm_roles_may_claim_a_workshop_certificate if {
	authz.allow with input as {"path": workshop_certificate, "method": "POST", "session": owner_session}
	authz.allow with input as {"path": workshop_certificate, "method": "POST", "session": admin_session}
	authz.allow with input as {"path": workshop_certificate, "method": "POST", "session": lawyer_session}
	authz.allow with input as {"path": workshop_certificate, "method": "POST", "session": clerk_session}
}

test_client_and_anonymous_may_not_claim_a_workshop_certificate if {
	not authz.allow with input as {"path": workshop_certificate, "method": "POST", "session": client_session}
	not authz.allow with input as {"path": workshop_certificate, "method": "POST", "session": null}
}

test_workshop_certificate_grant_is_exact if {
	not authz.allow with input as {"path": ["workshops"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["workshops", "use-the-navigator"], "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": workshop_certificate, "method": "GET", "session": lawyer_session}
	not authz.allow with input as {"path": ["presentations", "rust-in-peace", "certificate"], "method": "POST", "session": lawyer_session}
}

# The firm surface lives at /app/lawyer; the old /portal/admin prefix carries
# no lawyer allow rule. A lawyer GET there must NOT be allowed (the routes
# are gone, but if one ever returned the policy must not silently
# re-open it). Admin still passes via the bypass — that is the bypass
# rule's documented semantics, not a /portal/admin grant.
test_lawyer_denied_old_portal_admin if {
	not authz.allow with input as {"path": ["portal", "admin", "people"], "method": "GET", "session": lawyer_session}
}

# ---------- /mcp (lawyer tier only) ----------

test_lawyer_reaches_mcp if {
	authz.allow with input as {"path": ["mcp"], "method": "POST", "session": lawyer_session}
}

test_client_denied_on_mcp if {
	not authz.allow with input as {"path": ["mcp"], "method": "POST", "session": client_session}
}

test_clerk_denied_on_mcp if {
	not authz.allow with input as {"path": ["mcp"], "method": "POST", "session": clerk_session}
}

# ---------- /app/api/aida/rpc (lawyer tier only) ----------

test_lawyer_reaches_aida_rpc if {
	authz.allow with input as {"path": ["app", "api", "aida", "rpc"], "method": "POST", "session": lawyer_session}
}

test_client_denied_on_aida_rpc if {
	not authz.allow with input as {"path": ["app", "api", "aida", "rpc"], "method": "POST", "session": client_session}
}

test_clerk_denied_on_aida_rpc if {
	not authz.allow with input as {"path": ["app", "api", "aida", "rpc"], "method": "POST", "session": clerk_session}
}

# ---------- /app/api/* read paths ----------
#
# A signed-in caller is not automatically a permitted reader. These two used to
# assert the opposite — a `client` and a `clerk` both reached
# `GET /app/api/people` under a blanket any-authenticated grant. Each read is
# now named with its own tier, and the per-resource decisions live in the
# dedicated section further down; what stays here is the shape those tests
# replaced, so the reversal is visible where the old grant was asserted.

test_lawyer_get_api_allowed if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": lawyer_session}
}

test_client_get_api_denied if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": client_session}
}

test_anonymous_get_api_denied if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": null}
}

test_authenticated_post_api_denied if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "POST", "session": client_session}
}

# ---------- /app/api/people* command writes (lawyer tier only) ----------

test_lawyer_can_create_person if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "POST", "session": lawyer_session}
}

test_admin_can_create_person if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "POST", "session": admin_session}
}

test_owner_can_create_person if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "POST", "session": owner_session}
}

test_lawyer_can_update_person if {
	authz.allow with input as {"path": ["app", "api", "people", "p1"], "method": "PATCH", "session": lawyer_session}
}

test_lawyer_can_delete_person if {
	authz.allow with input as {"path": ["app", "api", "people", "p1"], "method": "DELETE", "session": lawyer_session}
}

test_lawyer_can_send_welcome if {
	authz.allow with input as {"path": ["app", "api", "people", "p1", "welcome"], "method": "POST", "session": lawyer_session}
}

test_client_denied_person_write if {
	not authz.allow with input as {"path": ["app", "api", "people", "p1"], "method": "PATCH", "session": client_session}
}

test_clerk_denied_person_write if {
	not authz.allow with input as {"path": ["app", "api", "people", "p1"], "method": "PATCH", "session": clerk_session}
}

test_anonymous_denied_person_write if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "POST", "session": null}
}

# ---------- /app/api/entities command writes (lawyer tier only) ----------

test_lawyer_can_create_entity if {
	authz.allow with input as {"path": ["app", "api", "entities"], "method": "POST", "session": lawyer_session}
}

test_admin_can_create_entity if {
	authz.allow with input as {"path": ["app", "api", "entities"], "method": "POST", "session": admin_session}
}

test_client_denied_entity_write if {
	not authz.allow with input as {"path": ["app", "api", "entities"], "method": "POST", "session": client_session}
}

test_clerk_denied_entity_write if {
	not authz.allow with input as {"path": ["app", "api", "entities"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_entity_write if {
	not authz.allow with input as {"path": ["app", "api", "entities"], "method": "POST", "session": null}
}

test_lawyer_can_update_entity if {
	authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "PATCH", "session": lawyer_session}
}

test_admin_can_update_entity if {
	authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "PATCH", "session": admin_session}
}

test_client_denied_entity_update if {
	not authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "PATCH", "session": client_session}
}

test_clerk_denied_entity_update if {
	not authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "PATCH", "session": clerk_session}
}

test_anonymous_denied_entity_update if {
	not authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "PATCH", "session": null}
}

test_lawyer_can_delete_entity if {
	authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "DELETE", "session": lawyer_session}
}

test_admin_can_delete_entity if {
	authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "DELETE", "session": admin_session}
}

test_client_denied_entity_delete if {
	not authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "DELETE", "session": client_session}
}

test_clerk_denied_entity_delete if {
	not authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "DELETE", "session": clerk_session}
}

test_anonymous_denied_entity_delete if {
	not authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "DELETE", "session": null}
}

# ---------- /app/api/projects/{id}/contract-review upload (any AUTHENTICATED tier — client-writable) ----------

test_client_can_upload_contract_review if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "contract-review"], "method": "POST", "session": client_session}
}

test_lawyer_can_upload_contract_review if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "contract-review"], "method": "POST", "session": lawyer_session}
}

test_admin_can_upload_contract_review if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "contract-review"], "method": "POST", "session": admin_session}
}

test_anonymous_denied_contract_review if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "contract-review"], "method": "POST", "session": null}
}

# ---------- /app/api/projects/{id}/participants add-participant command (lawyer tier only) ----------

test_lawyer_can_add_participant if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "POST", "session": lawyer_session}
}

test_admin_can_add_participant if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "POST", "session": admin_session}
}

test_owner_can_add_participant if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "POST", "session": owner_session}
}

test_client_denied_add_participant if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "POST", "session": client_session}
}

test_clerk_denied_add_participant if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_add_participant if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "POST", "session": null}
}

# ---------- PATCH/DELETE /app/api/projects/{id}/participants/{role_id} edit + remove (lawyer tier only) ----------

test_lawyer_can_edit_participant if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1"], "method": "PATCH", "session": lawyer_session}
}

test_admin_can_remove_participant if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1"], "method": "DELETE", "session": admin_session}
}

test_owner_can_edit_participant if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1"], "method": "PATCH", "session": owner_session}
}

test_client_denied_edit_participant if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1"], "method": "PATCH", "session": client_session}
}

test_clerk_denied_remove_participant if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1"], "method": "DELETE", "session": clerk_session}
}

test_anonymous_denied_remove_participant if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1"], "method": "DELETE", "session": null}
}

# ---------- PUT/DELETE /app/api/projects/{id}/participants/{role_id}/dri designate + clear DRI (lawyer tier only) ----------

test_lawyer_can_designate_dri if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1", "dri"], "method": "PUT", "session": lawyer_session}
}

test_admin_can_clear_dri if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1", "dri"], "method": "DELETE", "session": admin_session}
}

test_owner_can_designate_dri if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1", "dri"], "method": "PUT", "session": owner_session}
}

test_client_denied_designate_dri if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1", "dri"], "method": "PUT", "session": client_session}
}

test_clerk_denied_clear_dri if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1", "dri"], "method": "DELETE", "session": clerk_session}
}

test_anonymous_denied_designate_dri if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants", "r1", "dri"], "method": "PUT", "session": null}
}

# ---------- POST /app/api/projects/{id}/close open-closing-notation command (lawyer tier only) ----------

test_lawyer_can_close_matter if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "close"], "method": "POST", "session": lawyer_session}
}

test_admin_can_close_matter if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "close"], "method": "POST", "session": admin_session}
}

test_client_denied_close_matter if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "close"], "method": "POST", "session": client_session}
}

test_clerk_denied_close_matter if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "close"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_close_matter if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "close"], "method": "POST", "session": null}
}

# ---------- POST /app/api/projects/{id}/lifecycle direct transition command (lawyer tier only) ----------

test_lawyer_can_transition_matter if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "lifecycle"], "method": "POST", "session": lawyer_session}
}

test_admin_can_transition_matter if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "lifecycle"], "method": "POST", "session": admin_session}
}

test_client_denied_transition_matter if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "lifecycle"], "method": "POST", "session": client_session}
}

test_clerk_denied_transition_matter if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "lifecycle"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_transition_matter if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "lifecycle"], "method": "POST", "session": null}
}

# ---------- POST /app/api/projects/{id}/approve-plan (any AUTHENTICATED tier — client-writable) ----------

# The client approving their own estate plan reaches this door; the handler then
# enforces client-lens matter access.
test_client_can_approve_plan if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "approve-plan"], "method": "POST", "session": client_session}
}

test_lawyer_allowed_at_opa_approve_plan if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "approve-plan"], "method": "POST", "session": lawyer_session}
}

test_admin_allowed_at_opa_approve_plan if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "approve-plan"], "method": "POST", "session": admin_session}
}

test_clerk_allowed_at_opa_approve_plan if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "approve-plan"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_approve_plan if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "approve-plan"], "method": "POST", "session": null}
}

# ---------- POST /app/api/projects/{id}/conversation/messages (any AUTHENTICATED tier — client-writable) ----------

test_client_can_post_conversation_message if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation", "messages"], "method": "POST", "session": client_session}
}

test_lawyer_can_post_conversation_message if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation", "messages"], "method": "POST", "session": lawyer_session}
}

test_admin_can_post_conversation_message if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation", "messages"], "method": "POST", "session": admin_session}
}

test_clerk_allowed_at_opa_post_conversation_message if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation", "messages"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_post_conversation_message if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation", "messages"], "method": "POST", "session": null}
}

# ---------- POST /app/api/expunge-requests/{id}/authorize (ADMIN tier only — runs the expunge) ----------

test_admin_can_authorize_expunge if {
	authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "authorize"], "method": "POST", "session": admin_session}
}

# The one project-adjacent write the lawyer tier alone cannot fire.
test_lawyer_denied_authorize_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "authorize"], "method": "POST", "session": lawyer_session}
}

test_client_denied_authorize_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "authorize"], "method": "POST", "session": client_session}
}

test_clerk_denied_authorize_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "authorize"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_authorize_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "authorize"], "method": "POST", "session": null}
}

# ---------- GET /app/api/project-repositories (ADMIN tier only — reads every matter's row) ----------

test_admin_can_read_project_repositories if {
	authz.allow with input as {"path": ["app", "api", "project-repositories"], "method": "GET", "session": admin_session}
}

# The tier is the whole control. Every other matter read on this surface is
# participation-scoped, so a lawyer reaching an all-rows report would be the
# silent directory bypass `visible_projects_as_lawyer` refuses to grant.
test_lawyer_denied_project_repositories if {
	not authz.allow with input as {"path": ["app", "api", "project-repositories"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_project_repositories if {
	not authz.allow with input as {"path": ["app", "api", "project-repositories"], "method": "GET", "session": clerk_session}
}

test_client_denied_project_repositories if {
	not authz.allow with input as {"path": ["app", "api", "project-repositories"], "method": "GET", "session": client_session}
}

test_anonymous_denied_project_repositories if {
	not authz.allow with input as {"path": ["app", "api", "project-repositories"], "method": "GET", "session": null}
}

# The reason this door carries its own noun. Nested under `projects` the rule
# admitting any authenticated caller up to five segments would have reached it,
# so a client would have been policy-allowed on an admin-only report. Asserted
# so a future move back under that prefix fails here rather than in review.
test_a_client_reaches_a_projects_subpath_but_not_this_one if {
	authz.allow with input as {"path": ["app", "api", "projects", "reconcile"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["app", "api", "project-repositories"], "method": "GET", "session": client_session}
}

# ---------- GET /app/api/project-lifecycle (ADMIN tier only) ----------

test_admin_can_read_project_lifecycle if {
	authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": admin_session}
}

test_owner_can_read_project_lifecycle if {
	authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": owner_session}
}

test_lawyer_denied_project_lifecycle if {
	not authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_project_lifecycle if {
	not authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": clerk_session}
}

test_client_denied_project_lifecycle if {
	not authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": client_session}
}

test_anonymous_denied_project_lifecycle if {
	not authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": null}
}

test_a_client_reaches_a_projects_subpath_but_not_project_lifecycle if {
	authz.allow with input as {"path": ["app", "api", "projects", "lifecycle"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["app", "api", "project-lifecycle"], "method": "GET", "session": client_session}
}

# ---------- POST /app/api/project-surfaces/{id} (ADMIN tier only — provisions one matter's handles) ----------

test_admin_can_reconcile_project_surfaces if {
	authz.allow with input as {"path": ["app", "api", "project-surfaces", "p1"], "method": "POST", "session": admin_session}
}

test_lawyer_denied_project_surfaces if {
	not authz.allow with input as {"path": ["app", "api", "project-surfaces", "p1"], "method": "POST", "session": lawyer_session}
}

test_clerk_denied_project_surfaces if {
	not authz.allow with input as {"path": ["app", "api", "project-surfaces", "p1"], "method": "POST", "session": clerk_session}
}

test_client_denied_project_surfaces if {
	not authz.allow with input as {"path": ["app", "api", "project-surfaces", "p1"], "method": "POST", "session": client_session}
}

test_anonymous_denied_project_surfaces if {
	not authz.allow with input as {"path": ["app", "api", "project-surfaces", "p1"], "method": "POST", "session": null}
}

# Same noun-isolation as project-repositories: a GET nested under `projects`
# is policy-reachable by a client; this POST must not be.
test_a_client_reaches_a_projects_subpath_but_not_project_surfaces if {
	authz.allow with input as {"path": ["app", "api", "projects", "reconcile"], "method": "GET", "session": client_session}
	not authz.allow with input as {"path": ["app", "api", "project-surfaces", "p1"], "method": "POST", "session": client_session}
}

# ---------- POST /app/api/expunge-requests/{id}/deny (lawyer tier only) ----------

test_lawyer_can_deny_expunge if {
	authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "deny"], "method": "POST", "session": lawyer_session}
}

test_admin_can_deny_expunge if {
	authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "deny"], "method": "POST", "session": admin_session}
}

test_client_denied_deny_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "deny"], "method": "POST", "session": client_session}
}

test_clerk_denied_deny_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "deny"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_deny_expunge if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests", "e1", "deny"], "method": "POST", "session": null}
}

# ---------- POST /app/api/playbooks create playbook (lawyer tier only) ----------

test_lawyer_can_create_playbook if {
	authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "POST", "session": lawyer_session}
}

test_admin_can_create_playbook if {
	authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "POST", "session": admin_session}
}

test_client_denied_create_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "POST", "session": client_session}
}

test_clerk_denied_create_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_create_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "POST", "session": null}
}

# ---------- PUT /app/api/playbooks/{id} replace positions (lawyer tier only) ----------

test_lawyer_can_update_playbook if {
	authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "PUT", "session": lawyer_session}
}

test_admin_can_update_playbook if {
	authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "PUT", "session": admin_session}
}

test_client_denied_update_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "PUT", "session": client_session}
}

test_clerk_denied_update_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "PUT", "session": clerk_session}
}

test_anonymous_denied_update_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "PUT", "session": null}
}

# ---------- POST /app/api/contract-reviews/{id}/findings/{idx} save finding (lawyer tier only) ----------

test_lawyer_can_save_review_finding if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "findings", "0"], "method": "POST", "session": lawyer_session}
}

test_admin_can_save_review_finding if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "findings", "0"], "method": "POST", "session": admin_session}
}

test_client_denied_save_review_finding if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "findings", "0"], "method": "POST", "session": client_session}
}

test_clerk_denied_save_review_finding if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "findings", "0"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_save_review_finding if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "findings", "0"], "method": "POST", "session": null}
}

# ---------- POST /app/api/contract-reviews/{id}/summary edit summary (lawyer tier only) ----------

test_lawyer_can_save_review_summary if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "summary"], "method": "POST", "session": lawyer_session}
}

test_admin_can_save_review_summary if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "summary"], "method": "POST", "session": admin_session}
}

test_client_denied_save_review_summary if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "summary"], "method": "POST", "session": client_session}
}

test_clerk_denied_save_review_summary if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "summary"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_save_review_summary if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "summary"], "method": "POST", "session": null}
}

# ---------- POST /app/api/contract-reviews/{id}/approve approve review (lawyer tier only) ----------

test_lawyer_can_approve_review if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "approve"], "method": "POST", "session": lawyer_session}
}

test_admin_can_approve_review if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "approve"], "method": "POST", "session": admin_session}
}

test_client_denied_approve_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "approve"], "method": "POST", "session": client_session}
}

test_clerk_denied_approve_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "approve"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_approve_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "approve"], "method": "POST", "session": null}
}

# ---------- POST /app/api/contract-reviews/{id}/reject reject review (lawyer tier only) ----------

test_lawyer_can_reject_review if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "reject"], "method": "POST", "session": lawyer_session}
}

test_admin_can_reject_review if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "reject"], "method": "POST", "session": admin_session}
}

test_client_denied_reject_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "reject"], "method": "POST", "session": client_session}
}

test_clerk_denied_reject_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "reject"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_reject_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1", "reject"], "method": "POST", "session": null}
}

# ---------- POST /app/api/projects/{id}/documents file a document (lawyer tier only) ----------

test_lawyer_can_upload_document if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "POST", "session": lawyer_session}
}

test_admin_can_upload_document if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "POST", "session": admin_session}
}

test_client_denied_upload_document if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "POST", "session": client_session}
}

test_clerk_denied_upload_document if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_upload_document if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "POST", "session": null}
}

# ---------- PATCH /app/api/projects/{id}/documents/{asset_id} visibility (lawyer tier only) ----------

test_lawyer_can_reconcile_document_visibility if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents", "a1"], "method": "PATCH", "session": lawyer_session}
}

test_admin_can_reconcile_document_visibility if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents", "a1"], "method": "PATCH", "session": admin_session}
}

test_client_denied_reconcile_document_visibility if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents", "a1"], "method": "PATCH", "session": client_session}
}

test_clerk_denied_reconcile_document_visibility if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents", "a1"], "method": "PATCH", "session": clerk_session}
}

test_anonymous_denied_reconcile_document_visibility if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents", "a1"], "method": "PATCH", "session": null}
}

# ---------- POST /app/api/notations/{id}/transcript coverage pass (lawyer tier only) ----------

test_lawyer_can_run_transcript_coverage if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "transcript"], "method": "POST", "session": lawyer_session}
}

test_admin_can_run_transcript_coverage if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "transcript"], "method": "POST", "session": admin_session}
}

test_client_denied_run_transcript_coverage if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "transcript"], "method": "POST", "session": client_session}
}

test_clerk_denied_run_transcript_coverage if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "transcript"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_run_transcript_coverage if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "transcript"], "method": "POST", "session": null}
}

# ---------- POST /app/api/projects/{id}/notations/{nid}/transcript estate intake (lawyer tier only) ----------

test_lawyer_can_file_estate_transcript if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations", "n1", "transcript"], "method": "POST", "session": lawyer_session}
}

test_admin_can_file_estate_transcript if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations", "n1", "transcript"], "method": "POST", "session": admin_session}
}

test_client_denied_file_estate_transcript if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations", "n1", "transcript"], "method": "POST", "session": client_session}
}

test_clerk_denied_file_estate_transcript if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations", "n1", "transcript"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_file_estate_transcript if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations", "n1", "transcript"], "method": "POST", "session": null}
}

# ---------- GET matter read clusters (#866 — any AUTHENTICATED tier; handler self-scopes) ----------

test_client_can_list_projects if {
	authz.allow with input as {"path": ["app", "api", "projects"], "method": "GET", "session": client_session}
}

test_lawyer_can_list_projects if {
	authz.allow with input as {"path": ["app", "api", "projects"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_projects if {
	authz.allow with input as {"path": ["app", "api", "projects"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_list_projects if {
	authz.allow with input as {"path": ["app", "api", "projects"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_projects if {
	not authz.allow with input as {"path": ["app", "api", "projects"], "method": "GET", "session": null}
}

test_client_can_get_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "GET", "session": client_session}
}

test_lawyer_can_get_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "GET", "session": lawyer_session}
}

test_admin_can_get_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_get_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_get_project if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "GET", "session": null}
}

test_client_can_list_participants if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "GET", "session": client_session}
}

test_lawyer_can_list_participants if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_participants if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_list_participants if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_participants if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "participants"], "method": "GET", "session": null}
}

test_client_can_list_project_notations if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "GET", "session": client_session}
}

test_lawyer_can_list_project_notations if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_project_notations if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_list_project_notations if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_project_notations if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "GET", "session": null}
}

test_client_can_get_notation if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1"], "method": "GET", "session": client_session}
}

test_lawyer_can_get_notation if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1"], "method": "GET", "session": lawyer_session}
}

test_admin_can_get_notation if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_get_notation if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_get_notation if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1"], "method": "GET", "session": null}
}

# ---------- GET firm-tool reads (#866 — lawyer tier only) ----------

test_lawyer_can_list_playbooks if {
	authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_playbooks if {
	authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "GET", "session": admin_session}
}

test_client_denied_list_playbooks if {
	not authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "GET", "session": client_session}
}

test_clerk_denied_list_playbooks if {
	not authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_playbooks if {
	not authz.allow with input as {"path": ["app", "api", "playbooks"], "method": "GET", "session": null}
}

test_lawyer_can_get_playbook if {
	authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "GET", "session": lawyer_session}
}

test_admin_can_get_playbook if {
	authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "GET", "session": admin_session}
}

test_client_denied_get_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "GET", "session": client_session}
}

test_clerk_denied_get_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_get_playbook if {
	not authz.allow with input as {"path": ["app", "api", "playbooks", "p1"], "method": "GET", "session": null}
}

test_lawyer_can_get_contract_review if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1"], "method": "GET", "session": lawyer_session}
}

test_admin_can_get_contract_review if {
	authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1"], "method": "GET", "session": admin_session}
}

test_client_denied_get_contract_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1"], "method": "GET", "session": client_session}
}

test_clerk_denied_get_contract_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_get_contract_review if {
	not authz.allow with input as {"path": ["app", "api", "contract-reviews", "r1"], "method": "GET", "session": null}
}

# ---------- GET matter documents + conversation (#866 — authenticated + handler-scoped) ----------

test_client_can_list_documents if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "GET", "session": client_session}
}

test_lawyer_can_list_documents if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_documents if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_list_documents if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_documents if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "documents"], "method": "GET", "session": null}
}

test_client_can_get_conversation if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation"], "method": "GET", "session": client_session}
}

test_lawyer_can_get_conversation if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation"], "method": "GET", "session": lawyer_session}
}

test_admin_can_get_conversation if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation"], "method": "GET", "session": admin_session}
}

test_clerk_allowed_at_opa_get_conversation if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_get_conversation if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "conversation"], "method": "GET", "session": null}
}

# ---------- GET expunge queue + review documents (#866 — lawyer tier, firm work product) ----------

test_lawyer_can_list_expunge_requests if {
	authz.allow with input as {"path": ["app", "api", "expunge-requests"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_expunge_requests if {
	authz.allow with input as {"path": ["app", "api", "expunge-requests"], "method": "GET", "session": admin_session}
}

test_client_denied_list_expunge_requests if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests"], "method": "GET", "session": client_session}
}

test_clerk_denied_list_expunge_requests if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_expunge_requests if {
	not authz.allow with input as {"path": ["app", "api", "expunge-requests"], "method": "GET", "session": null}
}

test_lawyer_can_list_review_documents if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "review-documents"], "method": "GET", "session": lawyer_session}
}

test_admin_can_list_review_documents if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "review-documents"], "method": "GET", "session": admin_session}
}

test_client_denied_list_review_documents if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "review-documents"], "method": "GET", "session": client_session}
}

test_clerk_denied_list_review_documents if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "review-documents"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_list_review_documents if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "review-documents"], "method": "GET", "session": null}
}

# ---------- POST /app/api/projects/{id}/notations open-notation command (lawyer tier only) ----------

test_lawyer_can_create_notation if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "POST", "session": lawyer_session}
}

test_admin_can_create_notation if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "POST", "session": admin_session}
}

test_client_denied_create_notation if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "POST", "session": client_session}
}

test_clerk_denied_create_notation if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_create_notation if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1", "notations"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/answers answer-questionnaire-step command (lawyer tier only) ----------

test_lawyer_can_answer_notation_step if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "answers"], "method": "POST", "session": lawyer_session}
}

test_admin_can_answer_notation_step if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "answers"], "method": "POST", "session": admin_session}
}

test_client_denied_answer_notation_step if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "answers"], "method": "POST", "session": client_session}
}

test_clerk_denied_answer_notation_step if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "answers"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_answer_notation_step if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "answers"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/request-changes send-back command (lawyer tier only) ----------

test_lawyer_can_request_changes if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "request-changes"], "method": "POST", "session": lawyer_session}
}

test_admin_can_request_changes if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "request-changes"], "method": "POST", "session": admin_session}
}

test_client_denied_request_changes if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "request-changes"], "method": "POST", "session": client_session}
}

test_clerk_denied_request_changes if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "request-changes"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_request_changes if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "request-changes"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/reask resubmit command (lawyer tier only) ----------

test_lawyer_can_resubmit_reask if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "reask"], "method": "POST", "session": lawyer_session}
}

test_admin_can_resubmit_reask if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "reask"], "method": "POST", "session": admin_session}
}

test_client_denied_resubmit_reask if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "reask"], "method": "POST", "session": client_session}
}

test_clerk_denied_resubmit_reask if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "reask"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_resubmit_reask if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "reask"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/intake send-intake-link command (lawyer tier only) ----------

test_lawyer_can_send_notation_intake if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "intake"], "method": "POST", "session": lawyer_session}
}

test_admin_can_send_notation_intake if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "intake"], "method": "POST", "session": admin_session}
}

test_client_denied_send_notation_intake if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "intake"], "method": "POST", "session": client_session}
}

test_clerk_denied_send_notation_intake if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "intake"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_send_notation_intake if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "intake"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/approval approve-notation command (lawyer tier only) ----------

test_lawyer_can_approve_notation if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "approval"], "method": "POST", "session": lawyer_session}
}

test_admin_can_approve_notation if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "approval"], "method": "POST", "session": admin_session}
}

test_client_denied_approve_notation if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "approval"], "method": "POST", "session": client_session}
}

test_clerk_denied_approve_notation if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "approval"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_approve_notation if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "approval"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/signature send-for-signature command (lawyer tier only) ----------

test_lawyer_can_send_notation_signature if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "signature"], "method": "POST", "session": lawyer_session}
}

test_admin_can_send_notation_signature if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "signature"], "method": "POST", "session": admin_session}
}

test_client_denied_send_notation_signature if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "signature"], "method": "POST", "session": client_session}
}

test_clerk_denied_send_notation_signature if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "signature"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_send_notation_signature if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "signature"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/release-drafts release-drafts command (lawyer tier only) ----------

test_lawyer_can_release_drafts if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "release-drafts"], "method": "POST", "session": lawyer_session}
}

test_admin_can_release_drafts if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "release-drafts"], "method": "POST", "session": admin_session}
}

test_client_denied_release_drafts if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "release-drafts"], "method": "POST", "session": client_session}
}

test_clerk_denied_release_drafts if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "release-drafts"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_release_drafts if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "release-drafts"], "method": "POST", "session": null}
}

# ---------- POST /app/api/notations/{id}/clauses append-clause command (lawyer tier only) ----------

test_lawyer_can_add_clause if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses"], "method": "POST", "session": lawyer_session}
}

test_admin_can_add_clause if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses"], "method": "POST", "session": admin_session}
}

test_client_denied_add_clause if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses"], "method": "POST", "session": client_session}
}

test_clerk_denied_add_clause if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_add_clause if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses"], "method": "POST", "session": null}
}

# ---------- PATCH/DELETE /app/api/notations/{id}/clauses/{cid} + POST .../move (lawyer tier only) ----------

test_lawyer_can_edit_clause if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses", "c1"], "method": "PATCH", "session": lawyer_session}
}

test_admin_can_delete_clause if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses", "c1"], "method": "DELETE", "session": admin_session}
}

test_lawyer_can_move_clause if {
	authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses", "c1", "move"], "method": "POST", "session": lawyer_session}
}

test_client_denied_edit_clause if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses", "c1"], "method": "PATCH", "session": client_session}
}

test_client_denied_move_clause if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses", "c1", "move"], "method": "POST", "session": client_session}
}

test_anonymous_denied_delete_clause if {
	not authz.allow with input as {"path": ["app", "api", "notations", "n1", "clauses", "c1"], "method": "DELETE", "session": null}
}

# ---------- POST /app/api/review-documents/{id}/comments (any AUTHENTICATED tier — client-writable) ----------

# The new case: a CLIENT may reach this door (the handler then enforces
# client-lens matter scope). Every other /api write is lawyer-only.
test_client_can_add_review_comment if {
	authz.allow with input as {"path": ["app", "api", "review-documents", "d1", "comments"], "method": "POST", "session": client_session}
}

test_lawyer_can_add_review_comment if {
	authz.allow with input as {"path": ["app", "api", "review-documents", "d1", "comments"], "method": "POST", "session": lawyer_session}
}

test_admin_can_add_review_comment if {
	authz.allow with input as {"path": ["app", "api", "review-documents", "d1", "comments"], "method": "POST", "session": admin_session}
}

# A Clerk clears the policy layer (any authenticated), but the handler fails it
# closed (Clerk has no review-comment capability yet).
test_clerk_allowed_at_opa_add_review_comment if {
	authz.allow with input as {"path": ["app", "api", "review-documents", "d1", "comments"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_add_review_comment if {
	not authz.allow with input as {"path": ["app", "api", "review-documents", "d1", "comments"], "method": "POST", "session": null}
}

# ---------- POST /app/api/documents/{id}/deletion-requests (any AUTHENTICATED tier — client-writable) ----------

test_client_can_request_document_deletion if {
	authz.allow with input as {"path": ["app", "api", "documents", "d1", "deletion-requests"], "method": "POST", "session": client_session}
}

test_lawyer_can_request_document_deletion if {
	authz.allow with input as {"path": ["app", "api", "documents", "d1", "deletion-requests"], "method": "POST", "session": lawyer_session}
}

test_admin_can_request_document_deletion if {
	authz.allow with input as {"path": ["app", "api", "documents", "d1", "deletion-requests"], "method": "POST", "session": admin_session}
}

test_anonymous_denied_request_document_deletion if {
	not authz.allow with input as {"path": ["app", "api", "documents", "d1", "deletion-requests"], "method": "POST", "session": null}
}

# ---------- PATCH /app/api/projects/{id} descriptive update (lawyer tier only) ----------

test_lawyer_can_update_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "PATCH", "session": lawyer_session}
}

test_admin_can_update_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "PATCH", "session": admin_session}
}

test_client_denied_project_update if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "PATCH", "session": client_session}
}

test_clerk_denied_project_update if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "PATCH", "session": clerk_session}
}

test_anonymous_denied_project_update if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "PATCH", "session": null}
}

test_lawyer_can_delete_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "DELETE", "session": lawyer_session}
}

test_admin_can_delete_project if {
	authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "DELETE", "session": admin_session}
}

test_client_denied_project_delete if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "DELETE", "session": client_session}
}

test_clerk_denied_project_delete if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "DELETE", "session": clerk_session}
}

test_anonymous_denied_project_delete if {
	not authz.allow with input as {"path": ["app", "api", "projects", "p1"], "method": "DELETE", "session": null}
}

# ---------- POST /app/api/projects matter open (lawyer tier only) ----------

test_lawyer_can_open_matter if {
	authz.allow with input as {"path": ["app", "api", "projects"], "method": "POST", "session": lawyer_session}
}

test_admin_can_open_matter if {
	authz.allow with input as {"path": ["app", "api", "projects"], "method": "POST", "session": admin_session}
}

test_client_denied_matter_open if {
	not authz.allow with input as {"path": ["app", "api", "projects"], "method": "POST", "session": client_session}
}

test_clerk_denied_matter_open if {
	not authz.allow with input as {"path": ["app", "api", "projects"], "method": "POST", "session": clerk_session}
}

test_anonymous_denied_matter_open if {
	not authz.allow with input as {"path": ["app", "api", "projects"], "method": "POST", "session": null}
}

# A write to an /api resource that hasn't moved onto the command boundary
# yet stays denied — the per-resource rules are scoped to their resource,
# not a blanket /api write grant.
test_lawyer_denied_unmigrated_api_write if {
	not authz.allow with input as {"path": ["app", "api", "jurisdictions"], "method": "POST", "session": lawyer_session}
}

# ---------- stateless Template markdown validator (POST /app/api/templates/validate) ----------
# Linting Template markdown is a lawyer authoring activity — the same
# lawyer-tier gate as every other /app/api/* write/command. The canonical
# route is /app/api/templates/validate; the old /app/api/notations/validate
# alias is gone and must stay denied for everyone (see below).

test_lawyer_can_validate_template if {
	authz.allow with input as {"path": ["app", "api", "templates", "validate"], "method": "POST", "session": lawyer_session}
}

test_admin_can_validate_template if {
	authz.allow with input as {"path": ["app", "api", "templates", "validate"], "method": "POST", "session": admin_session}
}

test_client_denied_template_validate if {
	not authz.allow with input as {"path": ["app", "api", "templates", "validate"], "method": "POST", "session": client_session}
}

test_anonymous_denied_template_validate if {
	not authz.allow with input as {"path": ["app", "api", "templates", "validate"], "method": "POST", "session": null}
}

# The undocumented /app/api/notations/validate alias was removed. It carries
# no allow rule anymore, so even a lawyer caller falls through to
# default-deny — proving the alias is gone, not merely undocumented.
test_removed_notations_validate_alias_denied_for_lawyer if {
	not authz.allow with input as {"path": ["app", "api", "notations", "validate"], "method": "POST", "session": lawyer_session}
}

test_removed_notations_validate_alias_denied_for_anonymous if {
	not authz.allow with input as {"path": ["app", "api", "notations", "validate"], "method": "POST", "session": null}
}

# ---------- API documentation surfaces (decided by routing, not this policy) ----------

# The Swagger shell at /app/api (and its shorter public alias /api) and the
# OpenAPI document beside it are public: `portal::bootstrap` mounts all three
# with no session boundary and no `require_policy` layer at all, so this
# policy never evaluates a request for them — see the note above
# `/app/api/aida.json`'s own such rule, and `portal/tests/router_contract.rs`
# for the router-level half this Rego test cannot see. Mirrored here like the
# A2A agent card below: an anonymous read must not be allowed by this policy
# either, which is the only half a policy test can prove.
test_policy_does_not_decide_api_docs if {
	not authz.allow with input as {"path": ["app", "api"], "method": "GET", "session": null}
}

test_policy_does_not_decide_openapi_json if {
	not authz.allow with input as {"path": ["app", "api", "openapi.json"], "method": "GET", "session": null}
}

# The retired top-level paths. Nothing mounts at /api-docs or /openapi.json now
# that the documentation lives under /app/api, and the policy must not carry an
# exemption that would silently re-open them if a route ever returned there.
test_anonymous_denied_retired_api_docs if {
	not authz.allow with input as {"path": ["api-docs"], "method": "GET", "session": null}
}

test_anonymous_denied_retired_openapi if {
	not authz.allow with input as {"path": ["openapi.json"], "method": "GET", "session": null}
}

# The A2A agent card. It is gated by the session boundary rather than by a rule
# here, so an anonymous read must not be allowed by this policy either.
test_policy_does_not_decide_aida_card if {
	not authz.allow with input as {"path": ["app", "api", "aida.json"], "method": "GET", "session": null}
}

# ---------------------------------------------------------------------------
# /app/api read paths — one decision per resource, no blanket grant
# ---------------------------------------------------------------------------

# The CRM directory. Collection and item are asserted separately: the rule
# matches on a prefix, so a regression that narrowed it to the collection would
# leave `/app/api/people/{id}` silently unreachable.
test_lawyer_reads_people_collection if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": lawyer_session}
}

test_lawyer_reads_one_person if {
	authz.allow with input as {"path": ["app", "api", "people", "p1"], "method": "GET", "session": lawyer_session}
}

test_admin_reads_people_collection if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": admin_session}
}

test_owner_reads_people_collection if {
	authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": owner_session}
}

test_lawyer_reads_entities if {
	authz.allow with input as {"path": ["app", "api", "entities"], "method": "GET", "session": lawyer_session}
}

test_lawyer_reads_one_entity if {
	authz.allow with input as {"path": ["app", "api", "entities", "e1"], "method": "GET", "session": lawyer_session}
}

# The reason this whole section exists. Before the per-resource rules, a single
# any-authenticated GET grant admitted a `client` to the firm's entire people
# and entities directory, and the read handlers carry no tier check of their
# own — so this rule was the only thing standing there.
test_client_denied_on_people_directory if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": client_session}
}

test_client_denied_on_one_person if {
	not authz.allow with input as {"path": ["app", "api", "people", "p1"], "method": "GET", "session": client_session}
}

test_client_denied_on_entities if {
	not authz.allow with input as {"path": ["app", "api", "entities"], "method": "GET", "session": client_session}
}

test_clerk_denied_on_people_directory if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": clerk_session}
}

test_anonymous_denied_on_people_directory if {
	not authz.allow with input as {"path": ["app", "api", "people"], "method": "GET", "session": null}
}

# Reference data takes the same lawyer gate as the directory.
test_lawyer_reads_jurisdictions if {
	authz.allow with input as {"path": ["app", "api", "jurisdictions"], "method": "GET", "session": lawyer_session}
}

test_lawyer_reads_entity_types if {
	authz.allow with input as {"path": ["app", "api", "entity-types"], "method": "GET", "session": lawyer_session}
}

test_client_denied_on_jurisdictions if {
	not authz.allow with input as {"path": ["app", "api", "jurisdictions"], "method": "GET", "session": client_session}
}

test_client_denied_on_entity_types if {
	not authz.allow with input as {"path": ["app", "api", "entity-types"], "method": "GET", "session": client_session}
}

# Raw Template markdown stays open to every authenticated tier, because the
# notation and Template galleries link it and both admit `client`. This is the
# one read on the surface whose grant is deliberately wide, so a `client` allow
# here is the assertion rather than an oversight.
test_client_reads_raw_template_markdown if {
	authz.allow with input as {
		"path": ["app", "api", "templates", "neon-law", "shared", "retainer"],
		"method": "GET",
		"session": client_session,
	}
}

test_lawyer_reads_raw_template_markdown if {
	authz.allow with input as {
		"path": ["app", "api", "templates", "neon-law", "shared", "retainer"],
		"method": "GET",
		"session": lawyer_session,
	}
}

test_anonymous_denied_on_raw_template_markdown if {
	not authz.allow with input as {
		"path": ["app", "api", "templates", "neon-law", "shared", "retainer"],
		"method": "GET",
		"session": null,
	}
}

# The wide Template read must not widen the authoring command beside it: a
# client may read a template's source and still not lint a draft.
test_client_denied_on_template_validate_despite_the_read_grant if {
	not authz.allow with input as {"path": ["app", "api", "templates", "validate"], "method": "POST", "session": client_session}
}

# A GET route nobody has written a rule for gets no decision. This is what the
# blanket grant used to prevent from being true, and it is the property that
# makes adding a read endpoint fail closed.
test_an_unnamed_api_read_is_denied if {
	not authz.allow with input as {"path": ["app", "api", "invoices"], "method": "GET", "session": lawyer_session}
}

# ---------- /app/docs ----------
# The workspace documentation inside the application. Every tier that operates
# Navigator reads it; `client` is the one authenticated tier denied. `/docs`
# itself carries no rule in this policy — it sits behind the session boundary
# alone — so this is a second, role-restricted door rather than a gate closing
# over material that used to be open.

test_lawyer_reaches_app_docs if {
	authz.allow with input as {"path": ["app", "docs"], "method": "GET", "session": lawyer_session}
}

test_clerk_reaches_app_docs if {
	authz.allow with input as {"path": ["app", "docs"], "method": "GET", "session": clerk_session}
}

test_owner_reaches_app_docs if {
	authz.allow with input as {"path": ["app", "docs"], "method": "GET", "session": owner_session}
}

test_admin_reaches_app_docs if {
	authz.allow with input as {"path": ["app", "docs"], "method": "GET", "session": admin_session}
}

# One document beneath the hub carries the same audience as the hub itself.
test_lawyer_reaches_a_document_in_app_docs if {
	authz.allow with input as {"path": ["app", "docs", "glossary"], "method": "GET", "session": lawyer_session}
}

test_clerk_reaches_a_document_in_app_docs if {
	authz.allow with input as {"path": ["app", "docs", "glossary"], "method": "GET", "session": clerk_session}
}

# The denials. A client is authenticated and still refused: these documents
# describe firm-side operation, and the public `/docs` mount is their door.
test_client_denied_app_docs if {
	not authz.allow with input as {"path": ["app", "docs"], "method": "GET", "session": client_session}
}

test_client_denied_a_document_in_app_docs if {
	not authz.allow with input as {"path": ["app", "docs", "glossary"], "method": "GET", "session": client_session}
}

test_anonymous_denied_app_docs if {
	not authz.allow with input as {"path": ["app", "docs"], "method": "GET", "session": null}
}

# ---------- /app/team ----------
# The firm team home. Same audience as `/app/docs`: every firm tier is
# admitted, with `client` the one authenticated tier denied.

test_lawyer_reaches_app_portal if {
	authz.allow with input as {"path": ["app", "team"], "method": "GET", "session": lawyer_session}
}

test_clerk_reaches_app_portal if {
	authz.allow with input as {"path": ["app", "team"], "method": "GET", "session": clerk_session}
}

test_owner_reaches_app_portal if {
	authz.allow with input as {"path": ["app", "team"], "method": "GET", "session": owner_session}
}

test_admin_reaches_app_portal if {
	authz.allow with input as {"path": ["app", "team"], "method": "GET", "session": admin_session}
}

# The denials. A client is authenticated and still refused: the firm team home
# is not part of the client-facing matter surface.
test_client_denied_app_portal if {
	not authz.allow with input as {"path": ["app", "team"], "method": "GET", "session": client_session}
}

test_anonymous_denied_app_portal if {
	not authz.allow with input as {"path": ["app", "team"], "method": "GET", "session": null}
}

# ---------- /app/brands ----------
# The house-of-brands home. Same audience as `/app/team`: every firm tier is
# admitted, with `client` the one authenticated tier denied.

test_lawyer_reaches_app_brands if {
	authz.allow with input as {"path": ["app", "brands"], "method": "GET", "session": lawyer_session}
}

test_clerk_reaches_app_brands if {
	authz.allow with input as {"path": ["app", "brands"], "method": "GET", "session": clerk_session}
}

test_owner_reaches_app_brands if {
	authz.allow with input as {"path": ["app", "brands"], "method": "GET", "session": owner_session}
}

test_admin_reaches_app_brands if {
	authz.allow with input as {"path": ["app", "brands"], "method": "GET", "session": admin_session}
}

test_client_denied_app_brands if {
	not authz.allow with input as {"path": ["app", "brands"], "method": "GET", "session": client_session}
}

test_anonymous_denied_app_brands if {
	not authz.allow with input as {"path": ["app", "brands"], "method": "GET", "session": null}
}

# ---------- /app/owner ----------
# Deployment-wide firm inventory. Owner only: the Owner/Admin route bypass
# does not apply here, so an Admin is denied the same way a Lawyer is.

test_owner_reaches_app_owner if {
	authz.allow with input as {"path": ["app", "owner"], "method": "GET", "session": owner_session}
}

test_admin_denied_app_owner if {
	not authz.allow with input as {"path": ["app", "owner"], "method": "GET", "session": admin_session}
}

test_lawyer_denied_app_owner if {
	not authz.allow with input as {"path": ["app", "owner"], "method": "GET", "session": lawyer_session}
}

test_clerk_denied_app_owner if {
	not authz.allow with input as {"path": ["app", "owner"], "method": "GET", "session": clerk_session}
}

test_client_denied_app_owner if {
	not authz.allow with input as {"path": ["app", "owner"], "method": "GET", "session": client_session}
}

test_anonymous_denied_app_owner if {
	not authz.allow with input as {"path": ["app", "owner"], "method": "GET", "session": null}
}
