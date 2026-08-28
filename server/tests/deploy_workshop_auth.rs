//! Grounding test for the "Sign-in" section of the deploy workshop.
//!
//! `web/content/workshops/navigator/DEPLOY.md` now teaches the auth
//! stance: Neon Law Navigator delegates identity to an OIDC-compatible provider
//! (Rauthy / Auth0 / Okta / GCP Identity Platform) and **never stores
//! a password**. That prose is a public promise, so nothing stops it
//! drifting from reality — a password column added to `persons`, a
//! hashing crate pulled into the graph, a renamed env var, a discovery
//! mechanism the code no longer uses.
//!
//! These tests pin the section to the code the same way
//! `third_party_catalog.rs` pins the vendor table and
//! `cli`'s `devx::gcp::deploy_workshop_prose_matches_the_dry_run_pipeline` pins
//! the provisioning steps: every claim the workshop makes about sign-in
//! must be true of the code that ships in this commit.

use std::path::Path;

use serde::Deserialize;

/// Read a repo-root file relative to this crate (`web/` → workspace root
/// is one level up), matching the convention `third_party_catalog.rs`
/// and the docs loader use.
fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} — {e}", path.display()))
}

/// The body of the workshop's `### Sign-in:` section — everything from
/// that heading to the next sibling `###`.
fn signin_section() -> String {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    let after = deploy
        .split_once("\n### Sign-in")
        .expect("DEPLOY.md must carry a `### Sign-in` section")
        .1;
    // Stop at the next *sibling* heading. Slicing to the next `##` would
    // run past three unrelated `###` sections, silently widening every
    // assertion below to prose the Sign-in section never makes.
    match after.split_once("\n### ") {
        Some((body, _)) => body.to_string(),
        None => after.to_string(),
    }
}

#[test]
fn every_oauth_env_var_the_workshop_names_exists_in_env_example() {
    // Each `OAUTH_*` token the prose prints must be a real key in the
    // committed env contract — so the four-variable wiring the workshop
    // teaches can't drift from `.env.example`.
    let section = signin_section();
    let env_example = repo_file(".env.example");

    let mut named: Vec<&str> = section
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| t.starts_with("OAUTH_"))
        .collect();
    named.sort_unstable();
    named.dedup();

    assert!(
        named.len() >= 4,
        "the workshop must name the four OAUTH_* variables that wire OIDC, found {named:?}",
    );
    for var in &named {
        assert!(
            env_example.contains(var),
            "DEPLOY.md names `{var}`, but `.env.example` has no such key — \
             the workshop has drifted from the env contract",
        );
    }
}

#[test]
fn workshop_oidc_mechanism_matches_the_oauth_code() {
    // The discovery URL and the two route paths the prose teaches must be
    // the same literals the OIDC flow actually uses, so the "how sign-in
    // works" narrative stays bound to `portal/src/oauth.rs`.
    let section = signin_section();
    let oauth_rs = repo_file("portal/src/oauth.rs");

    for token in [
        "/.well-known/openid-configuration",
        "/auth/login",
        "/auth/callback",
    ] {
        assert!(
            section.contains(token),
            "DEPLOY.md sign-in section must mention `{token}`",
        );
        assert!(
            oauth_rs.contains(token),
            "DEPLOY.md names `{token}`, but `portal/src/oauth.rs` does not use it — prose drifted from code",
        );
    }
}

#[test]
fn workshop_bootstrap_owner_guidance_matches_the_auth_and_admin_code() {
    let section = signin_section();
    let env_example = repo_file(".env.example");
    let oauth_rs = repo_file("portal/src/oauth.rs");
    let admin_rs = repo_file("portal/src/admin.rs");

    let variable = "NAVIGATOR_BOOTSTRAP_OWNER_EMAIL";
    for (source, label) in [
        (&section, "DEPLOY.md sign-in section"),
        (&env_example, ".env.example"),
        (&oauth_rs, "portal/src/oauth.rs"),
    ] {
        assert!(
            source.contains(variable),
            "{label} must name `{variable}` so the protected identity stays grounded",
        );
    }

    assert!(
        section.contains("/app/admin/people"),
        "DEPLOY.md sign-in section must name the admin people console",
    );
    assert!(
        admin_rs.contains("/app/admin/people"),
        "the workshop names an admin people route the application no longer mounts",
    );
    // Key on the sentence that makes the role-management claim, not on the
    // bare role words: those occur throughout the
    // section on their own and would keep this test green with the claim
    // deleted outright.
    assert!(
        section.contains("role among `owner`, `admin`, `lawyer`, `clerk`, and `client`"),
        "the workshop must name all five roles in authority order",
    );
    assert!(
        section.contains("Only an Owner can assign or modify Owner"),
        "the workshop must state that Admin cannot govern Owner",
    );
    assert!(
        section.contains("immutable"),
        "the workshop must explain that the bootstrap-Owner record cannot be edited",
    );
    assert!(
        section.contains("unset, empty, or whitespace-only value disables"),
        "the workshop must explain how to disable bootstrap-Owner creation",
    );
}

#[test]
fn workshop_role_rings_keep_lawyer_and_clerk_boundaries_explicit() {
    let deploy = repo_file("server/content/workshops/navigator/DEPLOY.md");
    for token in [
        "### Role rings: who can do what",
        "class=\"role-rings\"",
        "Client",
        "Clerk",
        "Lawyer",
        "Admin",
        "Owner",
        "Anonymous",
        "supervised **non-lawyer**",
        "**licensed lawyer**",
        "`owner > admin > lawyer > clerk > client`",
        "Clerks reach `/app/projects`",
        "disclosed lawyer DRI",
    ] {
        assert!(
            deploy.contains(token),
            "role-ring slide must preserve `{token}`: {deploy}"
        );
    }
}

#[test]
fn never_store_passwords_promise_holds_in_the_code() {
    // The workshop promises, in print, that Neon Law Navigator never stores a
    // password. Bind that promise to two facts in the source tree: the
    // `persons` entity has no password field, and no password-hashing
    // crate is in the dependency graph. The day someone adds either, the
    // promise is false and this test fails — forcing the doc and the
    // decision to be revisited together.
    let section = signin_section();
    assert!(
        section.contains("never store") || section.contains("never stores"),
        "the workshop section must state the no-password-storage promise",
    );

    // The person record lives in SurrealDB (#1093; ENG-19): its shape is the
    // `person` half of the schema file, and `store::persons` is the only
    // module that reads or writes it. Both have to stay clean.
    //
    // The schema is read one `person` line at a time rather than whole. The
    // file also defines `email_token`, whose `purpose` field asserts
    // `'password_reset'` — a magic link sent to a verified address, which is
    // the opposite of storing a password. Matching the whole file would fail
    // on that and teach the next reader to delete the test.
    let schema = repo_file("store/src/schema/navigator.surql").to_lowercase();
    let person_definition: String = schema
        .lines()
        .filter(|line| line.contains(" on person"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        person_definition.contains("email_lower"),
        "the `person` table definition was not found in the schema file — this guard is \
         matching nothing, which would pass silently",
    );
    for (what, source) in [
        ("the `person` table definition", &person_definition),
        (
            "`store::persons`",
            &repo_file("store/src/persons.rs").to_lowercase(),
        ),
    ] {
        assert!(
            !source.contains("password"),
            "{what} now mentions `password` — the workshop's no-password promise is broken",
        );
    }

    // The dependency fact is about what Navigator's OWN code can reach, so
    // it reads the manifests rather than `Cargo.lock`. A password-hashing
    // crate sitting in the lock file is not evidence of anything by itself:
    // SurrealDB carries argon2, bcrypt, scrypt and pbkdf2 to authenticate
    // its own database users (#1093), which is an engine authenticating to
    // itself, not Navigator storing a person's password. What WOULD break
    // the promise is a workspace crate declaring one, because that is the
    // only way our code could call it — so that is what is asserted.
    for manifest_path in workspace_manifests() {
        let manifest = repo_file(&manifest_path);
        for crate_name in ["argon2", "bcrypt", "scrypt", "pbkdf2", "password-hash"] {
            assert!(
                !declares_dependency(&manifest, crate_name),
                "{manifest_path} now declares `{crate_name}` — Neon Law Navigator is \
                 storing/hashing passwords, contradicting the deploy workshop. Update the \
                 workshop or reconsider the design.",
            );
        }
    }
}

/// A guard that reads manifests can go vacuous in two ways — an empty
/// member list, or a matcher that never matches — and either would let
/// the promise above pass without testing anything. Pin both against
/// facts that are true today.
#[test]
fn the_no_password_guard_cannot_pass_vacuously() {
    let manifests = workspace_manifests();
    for expected in ["Cargo.toml", "store/Cargo.toml", "server/Cargo.toml"] {
        assert!(
            manifests.iter().any(|m| m == expected),
            "{expected} must be scanned: {manifests:?}"
        );
    }

    // The matcher finds a real declaration, in each spelling the
    // workspace uses.
    assert!(declares_dependency("serde = { version = \"1\" }", "serde"));
    assert!(declares_dependency(
        "surrealdb.workspace = true",
        "surrealdb"
    ));
    assert!(declares_dependency("  argon2 = \"0.5\"", "argon2"));
    assert!(declares_dependency(
        &repo_file("store/Cargo.toml"),
        "surrealdb"
    ));

    // And does not fire on a mention that is not a declaration — the
    // name inside a comment, a feature list, or another crate's name.
    assert!(!declares_dependency(
        "# argon2 is deliberately absent",
        "argon2"
    ));
    assert!(!declares_dependency(
        "surrealdb = { features = [\"argon2\"] }",
        "argon2"
    ));
    assert!(!declares_dependency("argon2-kdf = \"1\"", "argon2"));
}

/// Every `Cargo.toml` in the workspace: the root plus each member named
/// by its `members` list.
fn workspace_manifests() -> Vec<String> {
    let root = repo_file("Cargo.toml");
    let members = root
        .split_once("members = [")
        .expect("the workspace manifest must list members")
        .1
        .split_once(']')
        .expect("the members list must be closed")
        .0;

    let mut manifests = vec!["Cargo.toml".to_string()];
    manifests.extend(
        members
            .split(',')
            .map(|entry| entry.trim().trim_matches('"'))
            .filter(|entry| !entry.is_empty())
            .map(|member| format!("{member}/Cargo.toml")),
    );
    manifests
}

/// Whether `manifest` declares `crate_name` as a dependency of its own.
///
/// Dependency keys start a line (`argon2 = ...`, `argon2.workspace = ...`,
/// `argon2 = { ... }`), so a leading-anchored match distinguishes a
/// declaration from the crate's name appearing inside a comment or a
/// feature list.
fn declares_dependency(manifest: &str, crate_name: &str) -> bool {
    manifest.lines().map(str::trim).any(|line| {
        line.strip_prefix(crate_name)
            .is_some_and(|rest| rest.starts_with(" =") || rest.starts_with('.'))
    })
}

#[test]
fn workshop_keeps_a_no_code_email_password_path_named() {
    // The email/password-without-Google guidance must keep naming a
    // hosted-login OIDC provider that works with zero code changes, so
    // the "no Google account required" promise can't silently collapse to
    // Google-only. Rauthy is that path and is the IdP the KIND loop
    // already runs.
    let section = signin_section();
    assert!(
        section.contains("Rauthy"),
        "the sign-in section must name Rauthy — the verified zero-code email/password OIDC path",
    );
    assert!(
        section.contains("email/password"),
        "the sign-in section must address the email/password (no-Google) front door",
    );
}

#[test]
fn local_rauthy_presenter_accounts_match_the_dev_seed_contract() {
    // A fresh KIND rehearsal relies on two joins: Rauthy username → email,
    // then email → the disposable Person and litigation participation. Keep the
    // first join structured here; store's environment test exercises the rows
    // and participation in a real database.
    let rauthy = repo_file("k8s/overlays/kind/rauthy/local-fixture.yaml");
    let config_map = serde_yaml::Deserializer::from_str(&rauthy)
        .filter_map(|doc| serde_yaml::Value::deserialize(doc).ok())
        .find(|doc| {
            doc.get("kind").and_then(serde_yaml::Value::as_str) == Some("ConfigMap")
                && doc
                    .get("metadata")
                    .and_then(|metadata| metadata.get("name"))
                    .and_then(serde_yaml::Value::as_str)
                    == Some("rauthy-bootstrap")
        })
        .expect("Rauthy bootstrap ConfigMap");
    let users_json = config_map
        .get("data")
        .and_then(|data| data.get("users.json"))
        .and_then(serde_yaml::Value::as_str)
        .expect("Rauthy users JSON");
    let users: serde_json::Value =
        serde_json::from_str(users_json).expect("valid Rauthy users JSON");

    for (username, email) in [
        ("owner", "owner@neonlaw.com"),
        ("admin", "admin@neonlaw.com"),
        ("lawyer", "lawyer@neonlaw.com"),
        ("clerk", "clerk@neonlaw.com"),
        ("client", "client@neonlaw.com"),
    ] {
        let user = users
            .as_array()
            .expect("Rauthy users")
            .iter()
            .find(|user| user["preferred_username"] == username)
            .unwrap_or_else(|| panic!("local Rauthy fixture must define `{username}`"));
        assert_eq!(user["email"], email, "email join for `{username}`");
        assert_eq!(
            user["password"]["Plain"], "password",
            "every KIND-only fixture account shares the password `password`",
        );
    }

    let seed = repo_file("store/src/seed.rs");
    for token in [
        "SAMPLE_LITIGATION_CODE: &str = \"sample-litigation\"",
        "lawyer@neonlaw.com",
        "\"client@neonlaw.com\"",
        "lawyer_id, \"attorney\"",
    ] {
        assert!(
            seed.contains(token),
            "the dev seed must keep the Rauthy mapping and litigation participation: missing `{token}`",
        );
    }
}
