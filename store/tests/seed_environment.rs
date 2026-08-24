//! Integration coverage for environment-aware seed orchestration.
//!
//! The sample-matter fixture is a `SurrealDB` projects-cluster concern.
//! These tests assert the public project and participation read seams.
//!
//! Every test drives [`store::seed::seed_environment_with`] rather than
//! `seed_environment`, so the deployment profile is an argument instead of a
//! read of process environment. A sourced `.devx/env` would otherwise decide
//! what these tests assert.

use std::sync::Arc;

use store::persons::{self, NewPerson, Role};
use store::projects::{self, DriSide, NewProject};
use store::test_support::mem_surreal;
use store::DeploymentEnvironment;

async fn storage() -> Arc<dyn cloud::StorageService> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "navigator-seed-env-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    Arc::new(cloud::FsStorage::new(dir).await.unwrap())
}

/// A deployment holding real client files carries no invented ones, and no
/// fixture people either. The production profile is the whole predicate:
/// `NAVIGATOR_SIMULATED_MATTERS` cannot widen this, because it decides only
/// whether the banner renders and writes nothing itself.
#[tokio::test]
async fn a_seed_without_sample_matters_has_no_disposable_projects_or_people() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    let report = store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Production,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    assert!(projects::all(&surreal).await.unwrap().is_empty());
    for email in ["lawyer@neonlaw.com", "client@neonlaw.com"] {
        assert!(
            persons::find_by_email_ci(&surreal, email)
                .await
                .unwrap()
                .is_none(),
            "a real-matter deployment must not contain {email}"
        );
    }
    assert_eq!(report.projects_inserted, 0);
}

/// The default follows the deployment profile, which is what makes an
/// unconfigured production deployment safe and a local boot useful.
///
/// This is the pairing the `store::config` unit tests cover in isolation,
/// asserted here against the seed it actually governs — because the failure
/// being guarded against is not a wrong boolean, it is invented clients in a
/// database of real ones.
#[tokio::test]
async fn the_profile_decides_when_nothing_says_otherwise() {
    assert!(
        store::sample_matters_from(DeploymentEnvironment::Dev, |_| None).unwrap(),
        "a dev boot has nothing but fixtures"
    );
    assert!(
        !store::sample_matters_from(DeploymentEnvironment::Production, |_| None).unwrap(),
        "an unconfigured production deployment seeds no invented matters"
    );
    assert!(
        store::sample_matters_from(DeploymentEnvironment::Production, |_| Some(
            "true".to_string()
        ))
        .unwrap(),
        "the persistent staging deployment runs the production profile and says so"
    );
}

/// The fixture opens all three sample matters, each with both DRIs, and
/// nothing else.
///
/// Three rather than one on purpose: one matter can only ever demonstrate one
/// shape of legal work, and the participation-scoped project list is not worth
/// looking at with a single row in it. The count is asserted exactly, so a
/// fourth matter added to the table has to come here and say so.
#[tokio::test]
async fn the_fixture_opens_the_three_sample_matters_with_dris() {
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
        .expect("the fixture client");
    let lawyer = persons::find_by_email_ci(&surreal, "lawyer@neonlaw.com")
        .await
        .unwrap()
        .expect("lawyer fixture");

    let codes = store::seed::sample_matter_codes();
    assert_eq!(
        codes,
        ["sample-litigation", "sample-transactional", "sample-estate"]
    );

    let dri = persons::find_by_email_ci(&surreal, store::seed::bootstrap_owner_email().as_str())
        .await
        .unwrap()
        .expect("bootstrap owner");

    for code in &codes {
        let matter = projects::find_by_code(&surreal, code)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("the `{code}` matter"));
        assert_eq!(matter.status, "open", "{code}");
        assert_eq!(
            matter.repository_url.as_deref(),
            store::seed::sample_matter_repository(code),
            "{code} records the repository its portal bundle is built from"
        );

        let participations = projects::participations_for_project(&surreal, matter.id)
            .await
            .unwrap();
        assert!(
            participations.iter().any(|row| {
                row.person_id == client.id && row.participation == "client" && row.is_client_dri
            }),
            "{code} has a client DRI"
        );
        // The disclosed lawyer participates; the *accountable* lawyer is the
        // deployment's bootstrap owner, which is what a Clerk resolves and what
        // a matter is answered for.
        assert!(
            participations
                .iter()
                .any(|row| { row.person_id == lawyer.id && row.participation == "attorney" }),
            "{code} discloses a licensed lawyer"
        );
        assert!(
            participations
                .iter()
                .any(|row| { row.person_id == dri.id && row.is_lawyer_dri }),
            "{code} names the bootstrap owner as lawyer DRI"
        );
    }

    assert_eq!(
        projects::all(&surreal).await.unwrap().len(),
        codes.len(),
        "the fixture opens exactly the matters it declares"
    );
}

/// The fixture Admin is deliberately given no participation on any of them.
///
/// That absence is the ENG-81 decision made visible: privileged reach is a
/// place an administrator navigates to, not a silent widening of a shared
/// route. Adding a row for Admin "so the demo looks complete" would delete the
/// one thing this fixture demonstrates about the authorization model, so the
/// absence is asserted rather than left as a gap somebody helpfully fills.
#[tokio::test]
async fn the_fixture_admin_participates_in_nothing() {
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

    let admin = persons::find_by_email_ci(&surreal, "admin@neonlaw.com")
        .await
        .unwrap()
        .expect("the fixture admin can sign in");

    for code in store::seed::sample_matter_codes() {
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
                .all(|row| row.person_id != admin.id),
            "{code} must not carry a row for the unassigned administrator"
        );
    }
}

/// Every matter gets its own client Entity, so no two share one.
///
/// A shared Entity would be a quiet lie about the data model — three unrelated
/// clients cannot be one legal person — and it would make the estate matter and
/// the widget company indistinguishable on any surface that reads the Entity
/// rather than the Project.
#[tokio::test]
async fn each_sample_matter_has_its_own_client_entity() {
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

    let mut entities = Vec::new();
    for code in store::seed::sample_matter_codes() {
        entities.push(
            projects::find_by_code(&surreal, code)
                .await
                .unwrap()
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("the `{code}` matter"))
                .entity_id,
        );
    }
    let distinct: std::collections::BTreeSet<_> = entities.iter().collect();
    assert_eq!(
        distinct.len(),
        entities.len(),
        "each sample matter is opened for its own client"
    );
}

#[tokio::test]
async fn the_fixture_is_idempotent_and_repairs_participation_drift() {
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
    let litigation = projects::find_by_code(&surreal, "sample-litigation")
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the litigation matter");
    let client = persons::find_by_email_ci(&surreal, "client@neonlaw.com")
        .await
        .unwrap()
        .expect("litigation client");
    let role = projects::participations_for_project(&surreal, litigation.id)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.person_id == client.id)
        .expect("client participation");
    projects::update_participation(&surreal, role.id, client.id, "paralegal")
        .await
        .unwrap();
    let before = projects::all(&surreal).await.unwrap().len();

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Dev,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    assert_eq!(projects::all(&surreal).await.unwrap().len(), before);
    let repaired = projects::participations_for_project(&surreal, litigation.id)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.person_id == client.id)
        .expect("repaired client participation");
    assert_eq!(repaired.participation, "client");
    assert!(repaired.is_client_dri);
}

#[tokio::test]
async fn the_fixture_does_not_claim_a_same_named_project() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    store::seed::seed_canonical(&surreal, &storage)
        .await
        .unwrap();
    let owner = persons::create(
        &surreal,
        &NewPerson::with_role(
            "Unrelated Litigant",
            "unrelated-litigation@example.com",
            Role::Client,
        ),
    )
    .await
    .unwrap();
    let squatter = projects::create(
        &surreal,
        &NewProject {
            code: "unrelated-litigation".into(),
            name: "Example Signal Labs v. Example Data Systems".into(),
            status: "closed".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    projects::designate_dri_in_surreal(&surreal, squatter.id, owner.id, DriSide::Client)
        .await
        .unwrap();

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Dev,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    assert_eq!(
        projects::find_by_id(&surreal, squatter.id)
            .await
            .unwrap()
            .expect("unrelated project survives")
            .status,
        "closed"
    );
    assert_ne!(
        projects::find_by_code(&surreal, "sample-litigation")
            .await
            .unwrap()
            .expect("the seeded litigation matter")
            .id,
        squatter.id
    );
}

/// The brand layer is the one seed besides the canonical set that reaches
/// production, so this asserts it there rather than in `dev`: a production
/// boot must carry every box we actually answer mail at.
///
/// Their being real is the point. `Address.yaml` used to sit in the disposable
/// sample-matter fixture, which supplies the local matter rows
/// existed only on a developer's laptop.
///
/// One boot carries them all. Each box stays keyed to the entity that holds it
/// — asserted by `each_entity_holds_only_its_own_box` below — which is the
/// boundary that actually matters for client mail.
#[tokio::test]
async fn a_production_boot_carries_every_box_we_answer_mail_at() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Production,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    // The mail centre itself is a row too, and `seed_mailrooms` synthesizes a
    // placeholder address for it because `mailrooms.address_id` is NOT NULL.
    // That placeholder is not a box anyone receives mail at, so it is filtered
    // out here rather than folded into the expected list.
    assert!(
        store::mailrooms::find_by_name(&surreal, "Ridgeview Mail Center")
            .await
            .unwrap()
            .is_some(),
        "a production boot carries the mail centre its boxes live in"
    );

    let mut boxes: Vec<String> = store::addresses::list_all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.line1)
        .filter(|line| !line.starts_with("(via mailroom:"))
        .collect();
    boxes.sort();
    assert_eq!(
        boxes,
        vec![
            "5150 Mae Anne Ave Ste 405-9002".to_string(),
            "5150 Mae Anne Ave Ste 405-9005".to_string(),
            "5150 Mae Anne Ave Ste 405-9011".to_string(),
        ],
        "a boot carries every box we hold and nothing else"
    );

    assert!(
        store::entities::find_by_name(&surreal, "Yakcobieus Industries PC")
            .await
            .unwrap()
            .is_some(),
        "and the Firm's own California law corporation"
    );
}

/// The retired partnership's boxes do not seed.
///
/// `Neon Law` held four of them, one per state it answered mail in, and
/// every one has left both the registry and the address layer. This is not
/// tidiness: within the Ridgeview Mail Center the box number is the whole
/// address, so a `405-9777` row surviving here would route mail to a box the
/// firm no longer rents — and the three out-of-state addresses would assert a
/// presence in states no current entity holds a box in.
#[tokio::test]
async fn the_retired_partnerships_boxes_do_not_seed() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Production,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    assert!(
        store::entities::find_by_name(&surreal, "Neon Law")
            .await
            .unwrap()
            .is_none(),
        "the retired partnership is out of the canonical registry"
    );

    let lines: Vec<String> = store::addresses::list_all(&surreal)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.line1)
        .collect();
    for retired in [
        "5150 Mae Anne Ave Ste 405-9777",
        "1990 N California Blvd Ste 800",
        "12 E 49th St 18th Floor",
        "720 Seneca St Ste 107-715",
    ] {
        assert!(
            !lines.iter().any(|line| line == retired),
            "{retired} belonged to the retired partnership and must not seed"
        );
    }
}

/// One layer, several entities: each box belongs to exactly one of them.
///
/// Every box we hold shares a street, a suite, and a ZIP at the same mail
/// center, so within that facility the box number is the whole address — a row
/// keyed to the wrong entity misdelivers one organization's mail to another
/// rather than bouncing, and "they all seeded" would look correct in any test
/// that only counts rows.
#[tokio::test]
async fn each_entity_holds_only_its_own_box() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Production,
        store::seed::BrandSeed::Neon,
    )
    .await
    .unwrap();

    for (name, expected) in [
        (
            store::seed::FIRM_ENTITY_NAME,
            "5150 Mae Anne Ave Ste 405-9002",
        ),
        ("shook.family", "5150 Mae Anne Ave Ste 405-9005"),
    ] {
        let entity = store::entities::find_by_name(&surreal, name)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{name} seeds"));
        let boxes: Vec<String> = store::addresses::for_entity(&surreal, entity.id)
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.line1)
            .collect();
        assert_eq!(
            boxes,
            vec![expected.to_string()],
            "{name} holds exactly its own box"
        );
    }
}

/// A white-label tenant is a deliberate "seed nothing": it runs our
/// application but is not us, so none of our entities' postal identities
/// belong in its database.
#[tokio::test]
async fn a_tenant_boot_carries_none_of_our_addresses() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    store::seed::seed_environment_with(
        &surreal,
        &storage,
        DeploymentEnvironment::Production,
        store::seed::BrandSeed::Tenant,
    )
    .await
    .unwrap();

    assert!(store::addresses::list_all(&surreal)
        .await
        .unwrap()
        .is_empty());
}

/// Re-running a boot is the ordinary case — every deployment seeds on every
/// start — so the brand layer has to be idempotent like the two around it.
#[tokio::test]
async fn the_brand_layer_is_idempotent_across_boots() {
    let surreal = mem_surreal().await;
    let storage = storage().await;

    for _ in 0..2 {
        store::seed::seed_environment_with(
            &surreal,
            &storage,
            DeploymentEnvironment::Production,
            store::seed::BrandSeed::Neon,
        )
        .await
        .unwrap();
    }

    let second = store::seed::seed_brand(&surreal, store::seed::BrandSeed::Neon)
        .await
        .unwrap();
    assert_eq!(second.addresses_inserted, 0);
    assert_eq!(
        second.entities_inserted, 0,
        "the brand layer seeds entities too, and re-seeding must not duplicate them"
    );
    // Three boxes plus the mail centre's own placeholder address. The mailroom
    // seed is find-or-create on a UNIQUE name, so a second boot must not mint
    // a second facility — nor a second placeholder address behind it.
    assert_eq!(second.mailrooms_inserted, 0);
    assert_eq!(store::addresses::list_all(&surreal).await.unwrap().len(), 4);
}

/// The `dev` portfolio's sample mail still arrives after the mail centre
/// moved layers.
///
/// `seed_letters` resolves its mailroom by name and *skips* a record it cannot
/// find, so moving `seed_mailrooms` from the sample-matter fixture into the
/// brand layer put a cross-layer ordering dependency between them. It holds
/// only because `seed_environment` applies the brand layer before the
/// portfolio; reverse those two calls and this suite still passes everywhere
/// except here, with the mail silently absent rather than an error.
#[tokio::test]
async fn the_dev_portfolios_mail_survives_the_mailroom_moving_layers() {
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

    let letters = store::letters::list_all(&surreal).await.unwrap();
    assert!(
        !letters.is_empty(),
        "a dev boot seeds the sample mail; an empty set means the mailroom \
         lookup silently skipped every record"
    );
    assert!(letters
        .iter()
        .any(|l| l.summary == "Notice of Intent to Lien"));
}
