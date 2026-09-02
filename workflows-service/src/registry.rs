//! The canonical list of Restate services this worker registers, split by
//! shape, plus the naming guarantee that keeps the ingress legible.
//!
//! `main.rs` binds exactly these services onto the one endpoint Restate
//! discovers. Two shapes live there:
//!
//! - **Durable workflows** — one-shot, keyed-by-idempotency invocations
//!   (`Archives`, `Heartbeat`, the billing workflows). Their names are
//!   `PascalCase`, so a deployment's discovered service list reads like a
//!   catalogue of named things. The naming test below shares the
//!   `rules::is_pascal_case` predicate so every registered name is checked
//!   against one canonical definition of `PascalCase`. (Template `.md`
//!   basenames follow the *separate* `snake_case` convention `N103` now
//!   enforces — different artifact, different rule.)
//! - **Virtual objects** — keyed, addressable services (`notation`, invoked as
//!   `/notation/<id>/...`). Restate names objects in lowercase by convention,
//!   so this one is a *deliberate* exception to the `PascalCase` rule, pinned
//!   here so a future lowercase *workflow* still fails the test.
//!
//! This registry is the single source of truth for those names; the
//! `registered_services` test guards it against drift from `main.rs`'s actual
//! `.bind(...)` calls so adding a service without recording its name — or
//! recording one with the wrong case — fails the build.

/// Durable workflow services bound by the worker. Each name MUST be `PascalCase`
/// (enforced by `workflow_service_names_are_pascal_case`). Keep in lockstep
/// with the `.bind(...)` calls in `main.rs`.
pub const WORKFLOW_SERVICES: &[&str] = &[
    "Archives",
    "BillingCanary",
    "BillingDigest",
    "ReconcileInvoices",
    "DriDigest",
    "Heartbeat",
    "GitHubAutomationHeartbeat",
    "DevxIssueTriage",
];

/// Virtual-object services bound by the worker. Lowercase kebab-case by
/// deliberate exception — a virtual object is addressed by key
/// (`/notation/<id>/...`, `/devx-pr/<repo>-<pr>/...`), not invoked as a one-shot
/// workflow, so it follows Restate's object-naming convention rather than the
/// `PascalCase` template convention.
pub const VIRTUAL_OBJECTS: &[&str] = &["notation", "devx-pr", "devx-guardrails", "project-slack"];

#[cfg(test)]
mod tests {
    use super::{VIRTUAL_OBJECTS, WORKFLOW_SERVICES};

    /// Every registered durable workflow is named `PascalCase`, checked with
    /// the canonical `rules::is_pascal_case` predicate so all registered names
    /// share one definition. (Template `.md` basenames follow the separate
    /// `snake_case` convention `N103` enforces — a different artifact.)
    #[test]
    fn workflow_service_names_are_pascal_case() {
        for name in WORKFLOW_SERVICES {
            assert!(
                rules::is_pascal_case(name),
                "registered workflow `{name}` is not PascalCase — Restate workflow names are \
                 PascalCase by convention"
            );
        }
    }

    /// The virtual-object carve-out is exactly that — a carve-out. Pin it to
    /// lowercase so a future *workflow* mistakenly added as a lowercase name
    /// still trips `workflow_service_names_are_pascal_case` instead of being
    /// quietly parked here.
    #[test]
    fn virtual_objects_are_the_documented_lowercase_exception() {
        for name in VIRTUAL_OBJECTS {
            assert!(
                !rules::is_pascal_case(name),
                "`{name}` is PascalCase — it should be a WORKFLOW_SERVICE, not a virtual object"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "virtual object `{name}` must be lowercase kebab-case"
            );
        }
    }

    /// No name appears twice, and no workflow is mis-filed as an object.
    #[test]
    fn registry_names_are_unique_across_both_lists() {
        let mut all: Vec<&str> = WORKFLOW_SERVICES
            .iter()
            .chain(VIRTUAL_OBJECTS.iter())
            .copied()
            .collect();
        all.sort_unstable();
        let len = all.len();
        all.dedup();
        assert_eq!(len, all.len(), "duplicate service name in the registry");
    }

    /// Drift guard: the registry must account for every service `main.rs`
    /// actually binds. Counting `.bind(` calls in the worker's source keeps the
    /// two in lockstep — add a `.bind(...)` without listing its name here (or
    /// vice versa) and this fails. Cheap, but it catches the one mistake that
    /// otherwise ships a service with no naming guarantee at all.
    #[test]
    fn registry_accounts_for_every_bound_service() {
        let main_rs = include_str!("main.rs");
        let bind_calls = main_rs.matches(".bind(").count();
        let registered = WORKFLOW_SERVICES.len() + VIRTUAL_OBJECTS.len();
        assert_eq!(
            bind_calls, registered,
            "main.rs binds {bind_calls} services but the registry lists {registered}; \
             update WORKFLOW_SERVICES / VIRTUAL_OBJECTS to match the `.bind(...)` calls"
        );
    }

    /// The shared image reaches every persistent environment, but only the
    /// automation home may register services that consume the one
    /// GitHub App webhook stream. Keep the branch visible in source so this
    /// inexpensive guard catches an accidental unconditional `DevX` bind.
    #[test]
    fn non_authoritative_deployments_do_not_bind_github_automation_services() {
        let main_rs = include_str!("main.rs");
        let authority_branch = main_rs
            .split_once("let server = if github_automation_home {")
            .expect("main must branch on the GitHub automation authority")
            .1;
        let (_, non_authoritative_branch) = authority_branch
            .split_once("} else {")
            .expect("main must branch on the GitHub automation authority");

        assert!(main_rs.contains("if github_automation_home"));
        for service in [
            "DevxIssueTriageService",
            "DevxPrService",
            "GitHubGuardrailsService",
            "GitHubAutomationHeartbeatService",
        ] {
            assert!(
                main_rs.contains(service),
                "authoritative branch must bind {service}"
            );
            assert!(
                !non_authoritative_branch.contains(service),
                "non-authoritative deployments must not bind {service}"
            );
        }
    }
}
