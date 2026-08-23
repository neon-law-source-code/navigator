//! Grounding tests for the *claims* the workshop decks make, as distinct from
//! the *commands* they publish.
//!
//! `cli/tests/workshop_command_grounding.rs` already pins every `navigator …`
//! invocation in the decks against the real binary, and
//! `deploy_workshop_environment.rs` pins `DEPLOY.md`'s environment matrix
//! against `.env.example`. Between them sits the gap this file closes: a claim
//! written as prose, about behavior the code decides, with nothing asserting
//! the two agree.
//!
//! That gap is not hypothetical. `CONTRIBUTE.md` promised for months that "any
//! repository on the `github.com` enterprise is in scope" for
//! `ops github setup`. The boundary stopped being host-only — it is a
//! `(host, organization)` pair now — and no test noticed, because the sentence
//! is prose rather than a command. The deck was still teaching a scope
//! *wider* than the code allows, which is the direction that costs an operator
//! an afternoon.
//!
//! The rule these tests follow: assert the deck against the **live value**, not
//! against a copy of it. Where a real API exists (`store::seed`,
//! `store::persons::Role`, `portal::oauth::post_login_landing`, the route
//! constants, `cloud::workspace`'s coordinate keys) the assertion goes through
//! it, so renaming the thing breaks the test. Reading source as text is the
//! fallback for a private item, and each such read carries an anti-vacuity
//! guard, because a text match that stops matching passes silently — the
//! failure mode `deploy_workshop_auth.rs` guards the same way.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use store::persons::{self, Role};
use store::projects;
use store::test_support::mem_surreal;
use store::DeploymentEnvironment;

/// Read a repo-root file relative to this crate (`server/` → workspace root is
/// one level up), matching the convention the other workshop tests use.
fn repo_file(rel: &str) -> String {
    let path = repo_path(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} — {e}", path.display()))
}

fn repo_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

const README: &str = "server/content/workshops/navigator/README.md";
const CONTRIBUTE: &str = "server/content/workshops/navigator/CONTRIBUTE.md";
const DEPLOY: &str = "server/content/workshops/navigator/DEPLOY.md";
const RUST_IN_PEACE: &str = "server/content/workshops/navigator/RUST_IN_PEACE.md";

/// Every deck the loader publishes, so a fifth one cannot be added without
/// deciding what it owes.
const DECKS: &[&str] = &[README, CONTRIBUTE, DEPLOY, RUST_IN_PEACE];

/// Collapse a markdown file's whitespace so a claim split across a reflowed
/// line still matches. The decks are hard-wrapped at 120 characters and `S102`
/// repacks them, so any assertion keyed on a sentence has to be wrap-agnostic
/// or it becomes a reflow tripwire instead of a claim test.
fn prose(markdown: &str) -> String {
    markdown.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A throwaway filesystem storage backend, one per test, matching
/// `store/tests/seed_environment.rs`. The seed publishes each matter's portal
/// document, so it needs somewhere to put it.
async fn storage() -> Arc<dyn cloud::StorageService> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "navigator-workshop-claims-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    Arc::new(cloud::FsStorage::new(dir).await.unwrap())
}

// ---------------------------------------------------------------------------
// The frontmatter contract, asserted uniformly across all four decks
// ---------------------------------------------------------------------------

/// One deck's frontmatter block, as `(key, value)` pairs.
///
/// Deliberately not a YAML parse: the assertion below is that the block exists
/// and carries three specific keys, and a parser would turn "no frontmatter at
/// all" into an `Err` several layers away from the claim being made.
fn frontmatter(markdown: &str) -> Vec<(String, String)> {
    let Some(rest) = markdown.strip_prefix("---\n") else {
        return Vec::new();
    };
    let Some((block, _)) = rest.split_once("\n---") else {
        return Vec::new();
    };
    block
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn frontmatter_value(markdown: &str, key: &str) -> Option<String> {
    frontmatter(markdown)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Every deck declares `kind: workshop` with a non-empty `title` and
/// `description`.
///
/// Uniformly, and that is the point. `README.md` was the one deck of four
/// carrying no frontmatter at all, which was not a style difference:
/// `rules::classify_source` reads the declared `kind:` and nothing else, so a
/// deck without one classifies as plain prose and silently skips `C001`/`C002`.
/// `S104` cannot catch it either — it returns early when there is no
/// frontmatter block to inspect. Asserting the contract here is what makes the
/// omission loud instead of invisible.
#[test]
fn every_deck_declares_the_content_page_frontmatter_contract() {
    for deck in DECKS {
        let markdown = repo_file(deck);
        let keys = frontmatter(&markdown);
        assert!(
            !keys.is_empty(),
            "{deck} carries no frontmatter block — it will classify as plain prose and skip \
             the content-page rules (`C001`, `C002`) entirely",
        );
        assert_eq!(
            frontmatter_value(&markdown, "kind").as_deref(),
            Some("workshop"),
            "{deck} must declare `kind: workshop` — the declared kind is the sole classifier",
        );
        for key in ["title", "description"] {
            let value = frontmatter_value(&markdown, key)
                .unwrap_or_else(|| panic!("{deck} must declare `{key}`"));
            assert!(
                !value.is_empty(),
                "{deck} declares an empty `{key}`, which `C00{}` forbids",
                if key == "title" { 1 } else { 2 },
            );
        }
    }
}

/// Each deck's frontmatter `title` is the title the loader actually renders.
///
/// The loader does not read frontmatter: `material_from_markdown` takes the
/// title from `NAVIGATOR_MANIFEST` and `strip_frontmatter` discards the block.
/// So the two can disagree indefinitely without anything breaking, and a reader
/// editing the frontmatter would reasonably expect the page to follow. Pinning
/// them together makes the frontmatter honest about the page it heads.
#[test]
fn each_deck_frontmatter_title_matches_the_title_the_loader_publishes() {
    let materials =
        portal::workshops::loader::load_navigator(&repo_path("server/content/workshops"))
            .expect("load the navigator workshop manifest");

    assert_eq!(
        materials.len(),
        DECKS.len(),
        "the loader publishes {} materials but this test knows {} decks: {:?}",
        materials.len(),
        DECKS.len(),
        materials.iter().map(|m| &m.slug).collect::<Vec<_>>(),
    );

    for material in &materials {
        // Find the deck file by its rendered title, then check the frontmatter
        // agrees. Keying on the title is what makes a mismatch a failure rather
        // than a silently skipped file.
        let matched = DECKS.iter().find(|deck| {
            frontmatter_value(&repo_file(deck), "title").as_deref() == Some(material.title.as_str())
        });
        assert!(
            matched.is_some(),
            "the loader publishes {:?} as {:?}, but no deck declares that `title:` in its \
             frontmatter — the block and the rendered page have drifted",
            material.slug,
            material.title,
        );
    }
}

// ---------------------------------------------------------------------------
// The governance boundary CONTRIBUTE.md teaches
// ---------------------------------------------------------------------------

/// The `### Ship through GitOps` section of `CONTRIBUTE.md`, slide face and
/// presenter notes, up to the next sibling heading.
fn gitops_section() -> String {
    let contribute = repo_file(CONTRIBUTE);
    let after = contribute
        .split_once("\n### Ship through GitOps")
        .expect("CONTRIBUTE.md must carry a `### Ship through GitOps` section")
        .1;
    match after.split_once("\n### ") {
        Some((body, _)) => body.to_string(),
        None => after.to_string(),
    }
}

/// The value of a `const NAME: &str = "…";` in a source file.
///
/// Used only for a private item a dependency edge cannot reach — `server` does
/// not depend on `cli`, and `NAVIGATOR_SLUG` is private to `github_setup` in
/// any case.
fn source_str_const(source: &str, name: &str) -> Option<String> {
    let needle = format!("{name}: &str = \"");
    let start = source.find(&needle)? + needle.len();
    let rest = &source[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The deck's governance-scope claim names the coordinate the code enforces.
///
/// This is the test the stale sentence would have failed. Every token asserted
/// here is the **live value** of a public constant or a constant parsed out of
/// the module that enforces the boundary, so the deck cannot keep teaching a
/// coordinate that has been renamed or removed.
#[test]
fn the_gitops_deck_states_the_host_and_organization_boundary_the_cli_enforces() {
    let section = prose(&gitops_section());
    let github_setup = repo_file("cli/src/devx/github_setup.rs");

    // Both halves of the coordinate, by their real key names. A rename of
    // either fails here before it can go stale in print.
    for key in [
        cloud::workspace::NAVIGATOR_GIT_HOST,
        cloud::workspace::NAVIGATOR_GITHUB_ORG,
    ] {
        assert!(
            section.contains(key),
            "the GitOps section must name `{key}` — it is half of the (host, organization) pair \
             `ops github setup` governs",
        );
    }

    // The default host, as a value rather than a spelling.
    assert!(
        section.contains(cloud::workspace::DEFAULT_GIT_HOST),
        "the GitOps section must name the default host `{}`",
        cloud::workspace::DEFAULT_GIT_HOST,
    );

    // The always-admissible organization, derived the same way the code derives
    // it: the owner half of the slug naming Navigator itself.
    let slug = source_str_const(&github_setup, "NAVIGATOR_SLUG")
        .expect("github_setup.rs must define NAVIGATOR_SLUG");
    let public_organization = slug
        .split('/')
        .next()
        .expect("NAVIGATOR_SLUG is an owner/name slug");
    assert!(
        section.contains(public_organization),
        "the GitOps section must name `{public_organization}` as the organization that is \
         admissible on every run",
    );

    // The refusal ordering, which is still true and is the operator-visible
    // half of the promise: `RepositoryTarget::resolve` runs `admits` before
    // `GitHubClient::from_env` reads a token.
    assert!(
        section.contains("refused before a token is read"),
        "the GitOps section must keep the refusal-before-token promise",
    );
}

/// The boundary is still a pair, so the deck's claim is still the true one.
///
/// Two failure directions, both real. If the organization half is removed the
/// boundary collapses back to a host check and the deck over-promises again; if
/// the host half goes, the deck names a coordinate that no longer exists.
/// Either way this deck needs rewriting, and this is where that gets said.
#[test]
fn the_governance_boundary_is_still_a_host_and_organization_pair() {
    let github_setup = repo_file("cli/src/devx/github_setup.rs");

    for shape in [
        // The pair itself, and the organization half that makes it tighter
        // than a host check.
        "struct GovernedForge",
        "organizations: Vec<String>",
        "fn admits",
        // The public organization is derived from the slug, not written twice.
        "fn public_organization",
        "NAVIGATOR_SLUG",
        // The deployment's own half.
        "config.organization",
    ] {
        assert!(
            github_setup.contains(shape),
            "`github_setup.rs` no longer carries `{shape}` — the (host, organization) boundary \
             changed shape, so CONTRIBUTE.md's GitOps claim must be re-grounded",
        );
    }

    // The retired host-only boundary must not come back under its old name.
    assert!(
        !github_setup.contains("enterprise_host"),
        "`enterprise_host` is back — the host-only boundary was retired in #82 and the decks \
         were rewritten against the pair",
    );
}

/// No deck reintroduces the two spellings the stale claim was made of.
///
/// The word "enterprise" slipped both of ENG-284's greps because it is
/// lowercase and not adjacent to "host", and "the private `navigator` CLI"
/// outlived the AGPL relicensing in #7 for the same reason: nothing was looking.
/// `DEPLOY.md` is exempt for "enterprise" — it names GitHub Enterprise and
/// Gemini Enterprise as real products in the OIDC-issuer and MCP sections,
/// which is correct usage rather than drift.
#[test]
fn no_deck_calls_the_forge_an_enterprise_or_the_cli_private() {
    for deck in DECKS {
        let markdown = repo_file(deck);
        let text = prose(&markdown);

        assert!(
            !text.contains("private `navigator` CLI"),
            "{deck} calls the `navigator` CLI private — its source has been public since #7",
        );

        if *deck == DEPLOY || *deck == RUST_IN_PEACE {
            continue;
        }
        assert!(
            !text.to_lowercase().contains("enterprise"),
            "{deck} says \"enterprise\" — the forge is a (host, organization) pair, and the \
             host-only \"enterprise\" framing was retired in #82",
        );
    }
}

// ---------------------------------------------------------------------------
// The README fixture account table, against the seed the fixture writes
// ---------------------------------------------------------------------------

/// The `(email, role)` pairs the Using deck's sign-in table publishes.
///
/// Parsed out of the markdown table rather than restated, so the deck is the
/// input to the assertion and not a second copy of the answer.
fn readme_account_table() -> Vec<(String, Role)> {
    let readme = repo_file(README);
    let mut rows = Vec::new();
    for line in readme.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 3 {
            continue;
        }
        let email = cells[0].trim_matches('`');
        if !email.contains('@') {
            continue;
        }
        let role = Role::parse(cells[1]).unwrap_or_else(|| {
            panic!(
                "the Using deck's account table names role `{}`, which is not a stored role",
                cells[1]
            )
        });
        rows.push((email.to_string(), role));
    }
    rows
}

/// Every account the deck tells a reader to sign in as exists, with the role
/// the deck prints, in a database seeded exactly as a local boot seeds one.
///
/// The strongest form available: the seed runs, and the table is checked against
/// the rows it wrote. A renamed fixture account, a changed role, or a sixth
/// account added to the seed and not to the deck all fail here.
#[tokio::test]
async fn the_readme_account_table_matches_the_seeded_fixture_people() {
    let surreal = mem_surreal().await;
    let storage = storage().await;
    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Dev,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    let table = readme_account_table();
    assert_eq!(
        table.len(),
        5,
        "the Using deck must publish all five role-named accounts, found {table:?}",
    );

    for (email, role) in &table {
        let person = persons::find_by_email_ci(&surreal, email)
            .await
            .unwrap()
            .unwrap_or_else(|| {
                panic!("the Using deck sends a reader to sign in as `{email}`, which the fixture never seeds")
            });
        assert_eq!(
            person.role,
            *role,
            "the Using deck prints `{email}` as `{}`, but the fixture seeds it as `{}`",
            role.as_str(),
            person.role.as_str(),
        );
    }

    // Every stored role is represented, so the table teaches the whole
    // authorization model rather than a subset of it.
    let published: std::collections::BTreeSet<&str> =
        table.iter().map(|(_, role)| role.as_str()).collect();
    for role in [
        Role::Owner,
        Role::Admin,
        Role::Lawyer,
        Role::Clerk,
        Role::Client,
    ] {
        assert!(
            published.contains(role.as_str()),
            "the Using deck's table omits the `{}` tier",
            role.as_str(),
        );
    }
}

/// The password the deck prints is the password the KIND fixture sets.
///
/// `deploy_workshop_auth.rs` already pins the Rauthy side of this join for its
/// own deck; this asserts the Using deck makes the same promise, so a fixture
/// password change cannot leave one deck right and the other wrong.
#[test]
fn the_readme_publishes_the_password_the_rauthy_fixture_sets() {
    let rauthy = repo_file("k8s/overlays/kind/rauthy/local-fixture.yaml");
    let readme = prose(&repo_file(README));

    // The fixture's plain password, read off the bootstrap document.
    let marker = "\"Plain\":";
    let start = rauthy
        .find(marker)
        .expect("the Rauthy fixture must set a plain password")
        + marker.len();
    let rest = &rauthy[start..];
    let open = rest.find('"').expect("a quoted password value");
    let tail = &rest[open + 1..];
    let close = tail.find('"').expect("a closed password value");
    let password = &tail[..close];

    assert!(
        !password.is_empty(),
        "read an empty password off the Rauthy fixture — this guard is matching nothing",
    );
    assert!(
        readme.contains(&format!("the password `{password}`")),
        "the Using deck must print the fixture password `{password}`",
    );
}

/// The participation claim on the sign-in slide is true of the seeded rows.
///
/// The deck says the client, lawyer, clerk, and owner rows are part of the
/// fixture and that Admin is the local administrator who grants its own at
/// `/app/admin`. That is the ENG-81 decision the fixture exists to demonstrate,
/// and it is exactly the kind of claim that reads as an oversight and gets
/// "helpfully" fixed by adding an Admin row — which would make the deck wrong.
#[tokio::test]
async fn the_readme_participation_claim_matches_the_seeded_rows() {
    let surreal = mem_surreal().await;
    let storage = storage().await;
    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Dev,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    let mut people = Vec::new();
    for email in [
        "owner@neonlaw.com",
        "lawyer@neonlaw.com",
        "clerk@neonlaw.com",
        "client@neonlaw.com",
    ] {
        people.push((
            email,
            persons::find_by_email_ci(&surreal, email)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("the fixture {email}"))
                .id,
        ));
    }
    let admin = persons::find_by_email_ci(&surreal, "admin@neonlaw.com")
        .await
        .unwrap()
        .expect("the fixture admin can sign in")
        .id;

    let litigation = projects::find_by_code(&surreal, store::seed::SAMPLE_LITIGATION_CODE)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the litigation matter");
    let rows = projects::participations_for_project(&surreal, litigation.id)
        .await
        .unwrap();

    for (email, id) in &people {
        assert!(
            rows.iter().any(|row| row.person_id == *id),
            "the Using deck says the {email} row is part of the fixture, but the litigation \
             matter carries none",
        );
    }
    assert!(
        rows.iter().all(|row| row.person_id != admin),
        "the fixture Admin now participates in the litigation matter — the Using deck's \
         sign-in slide and its `/app/admin` instruction both assume it does not",
    );

    // And the deck says so in print, naming the administration surface a
    // reader is sent to.
    let readme = prose(&repo_file(README));
    assert!(
        readme.contains(portal::dioxus_app::ADMIN_LANDING_PATH),
        "the Using deck must name `{}` as where participation is granted",
        portal::dioxus_app::ADMIN_LANDING_PATH,
    );
}

/// The deck's "all three matters" claim is true for the client.
///
/// Derived from `sample_matter_codes()` rather than the number three, so a
/// fourth sample matter fails the deck's wording instead of silently narrowing
/// what this checks.
#[tokio::test]
async fn the_client_sees_every_sample_matter_the_deck_promises() {
    let surreal = mem_surreal().await;
    let storage = storage().await;
    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Dev,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    let client = persons::find_by_email_ci(&surreal, "client@neonlaw.com")
        .await
        .unwrap()
        .expect("the fixture client")
        .id;

    let codes = store::seed::sample_matter_codes();
    assert!(
        !codes.is_empty(),
        "no sample matters — this guard would pass vacuously",
    );
    for code in &codes {
        let matter = projects::find_by_code(&surreal, code)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("the `{code}` matter"));
        assert!(
            projects::participations_for_project(&surreal, matter.id)
                .await
                .unwrap()
                .iter()
                .any(|row| row.person_id == client),
            "the Using deck tells the room every seeded matter is in the client's list, but \
             `{code}` carries no client row",
        );
    }

    // The deck teaches the litigation matter by name and by practice; both come
    // off the seed rather than out of the prose.
    let readme = prose(&repo_file(README));
    let litigation = projects::find_by_code(&surreal, store::seed::SAMPLE_LITIGATION_CODE)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the litigation matter");
    assert!(
        readme.contains(&litigation.name),
        "the Using deck must name the matter it works, `{}`",
        litigation.name,
    );
    let practice = litigation
        .description
        .as_deref()
        .expect("the seed opens the litigation matter with a described practice");
    assert!(
        readme.contains(practice),
        "the Using deck describes the litigation matter as something other than the practice \
         the seed opens it for: `{practice}`",
    );
}

// ---------------------------------------------------------------------------
// The landing routes, asserted through the function that decides them
// ---------------------------------------------------------------------------

/// The route each role lands on after sign-in, checked against the real
/// decision function rather than narrated.
///
/// `post_login_landing` is the whole rule, so the assertion calls it with each
/// stored role and requires the deck to print the answer it gives. A test that
/// re-implemented the client/firm fork would agree with itself forever while
/// the deck drifted.
#[test]
fn the_readme_landing_routes_are_the_ones_post_login_landing_returns() {
    let readme = prose(&repo_file(README));

    // The neutral default: no `return_to`, so the tier landing decides.
    let client_landing = portal::oauth::post_login_landing(Role::Client, "");
    assert_eq!(
        client_landing,
        portal::dioxus_app::PROJECTS_PATH,
        "a client's landing is the matter list",
    );

    for role in [Role::Owner, Role::Admin, Role::Lawyer, Role::Clerk] {
        let landing = portal::oauth::post_login_landing(role, "");
        assert_eq!(
            landing,
            portal::dioxus_app::APP_TEAM_PATH,
            "the `{}` tier is a firm tier and lands on the team home",
            role.as_str(),
        );
        assert!(
            readme.contains(&landing),
            "the Using deck must name `{landing}`, where the `{}` tier lands",
            role.as_str(),
        );
    }

    assert!(
        readme.contains(&client_landing),
        "the Using deck must name `{client_landing}`, where the client tier lands",
    );
    assert_ne!(
        client_landing,
        portal::dioxus_app::APP_TEAM_PATH,
        "if the two tiers land in the same place the deck's sign-in slide is teaching a \
         distinction that no longer exists",
    );
}

/// The portal path the deck prints is the route the application mounts.
///
/// `PROJECT_PORTAL_ROOT` is private to `project_portal`, so this reads the
/// template out of the module and renders it with a real seeded code — which
/// catches a changed *shape* (a renamed segment, a moved parameter), not just a
/// changed string.
#[test]
fn the_readme_portal_path_is_the_route_the_application_mounts() {
    let readme = prose(&repo_file(README));
    let source = repo_file("portal/src/project_portal.rs");

    let template = source_str_const(&source, "PROJECT_PORTAL_ROOT")
        .expect("project_portal.rs must define PROJECT_PORTAL_ROOT");
    assert!(
        template.contains("{project_code}"),
        "the portal route template lost its `{{project_code}}` parameter — this guard is \
         matching nothing: {template}",
    );

    let concrete = template.replace("{project_code}", store::seed::SAMPLE_LITIGATION_CODE);
    assert!(
        readme.contains(&concrete),
        "the Using deck must print the portal path the router mounts, `{concrete}`",
    );

    // The matter-detail path the deck also prints, from the public constant.
    let detail = format!(
        "{}/{}",
        portal::dioxus_app::PROJECTS_PATH,
        store::seed::SAMPLE_LITIGATION_CODE
    );
    assert!(
        readme.contains(&detail),
        "the Using deck must print the matter detail path `{detail}`",
    );
}

/// The deck names the sample-project staging directory by the variable the
/// generated environment actually carries.
///
/// Grounded on `store::sample_project::STAGE_ENV` — the constant `web` reads and
/// `worktree-env up` writes — rather than on `.env.example`, which does not
/// document this key today. That omission is a real gap, but closing it means
/// adding a row to `DEPLOY.md`'s Environment Matrix (otherwise
/// `operating_workshop_lists_every_committed_environment_variable` fails on the
/// new key), and that is a deck edit rather than a test fix.
#[test]
fn the_readme_names_the_sample_projects_directory_variable() {
    let readme = prose(&repo_file(README));
    let variable = store::sample_project::STAGE_ENV;

    assert!(
        !variable.is_empty(),
        "STAGE_ENV is empty — this guard is matching nothing",
    );
    assert!(
        readme.contains(variable),
        "the Using deck must name `{variable}`, the directory it tells a reader to source",
    );
}

// ---------------------------------------------------------------------------
// Anti-vacuity
// ---------------------------------------------------------------------------

/// The helpers above can each go vacuous in a way that would let a stale claim
/// pass: a frontmatter parser that returns nothing, a table parser that finds no
/// rows, a const reader that misses. Pin all three against facts true today,
/// the way `deploy_workshop_auth.rs` pins its manifest scan.
#[test]
fn the_claim_guards_cannot_pass_vacuously() {
    // The frontmatter parser reads a real block, and reports absence as absence.
    assert_eq!(
        frontmatter_value("---\nkind: workshop\ntitle: T\n---\n\n# B\n", "title").as_deref(),
        Some("T"),
    );
    assert!(frontmatter("# B\n\nNo frontmatter.\n").is_empty());
    assert!(frontmatter_value(&repo_file(README), "kind").is_some());

    // The table parser finds the five published rows, and skips the separator.
    let table = readme_account_table();
    assert_eq!(table.len(), 5, "{table:?}");
    assert!(table
        .iter()
        .any(|(email, role)| email == "client@neonlaw.com" && *role == Role::Client));

    // The const reader finds a real value, and does not invent one.
    assert_eq!(
        source_str_const("const X: &str = \"hello\";", "X").as_deref(),
        Some("hello"),
    );
    assert_eq!(source_str_const("const X: &str = \"hello\";", "Y"), None);

    // Whitespace collapsing makes a reflowed claim matchable.
    assert_eq!(prose("a\n  b   c\n"), "a b c");
}
