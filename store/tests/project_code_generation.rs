//! Every matter-open door generates a suffixed code from the caller's stem.
//!
//! A hand-picked code can never change once chosen (`project.code` is
//! `READONLY`), so deriving one straight from a stem would make the first
//! matter to claim a stem the only one that ever could — two matters for one
//! client, or two clients with similar names, would permanently strand the
//! loser with whatever it settled for. `open_matter` closes that trap by
//! appending a generated suffix (`store::projects::code_from_name`) to every
//! stem, the same shape the self-serve retainer walk already used.

use store::persons::{self, NewPerson, Role};
use store::projects::{self, OpenMatterCommand};
use store::surreal::SurrealDb;
use store::test_support::{mem_surreal, seed_entity};
use uuid::Uuid;

/// Every reference `open_matter` validates, seeded: the client, the
/// attesting attorney, and the entity.
async fn references(surreal: &SurrealDb) -> (Uuid, Uuid, Uuid) {
    let client = persons::create(
        surreal,
        &NewPerson::with_role("Libra Client", "libra@example.com", Role::Client),
    )
    .await
    .expect("client");
    let attester = persons::create(
        surreal,
        &NewPerson::with_role("Virgo Attorney", "virgo@example.com", Role::Lawyer),
    )
    .await
    .expect("attester");
    let entity_id = seed_entity(surreal).await;
    (client.id, attester.id, entity_id)
}

fn command(
    name: &str,
    code: &str,
    client_id: Uuid,
    entity_id: Uuid,
    acting_person_id: Uuid,
) -> OpenMatterCommand {
    OpenMatterCommand {
        name: name.to_string(),
        code: code.to_string(),
        client_id,
        entity_id,
        description: None,
        attestation: true,
        acting_person_id,
    }
}

/// The stored code is the stem, a single hyphen, and an 8-letter suffix — not
/// the stem verbatim.
#[tokio::test]
async fn open_matter_appends_a_generated_suffix_to_the_supplied_stem() {
    let surreal = mem_surreal().await;
    let (client_id, acting_person_id, entity_id) = references(&surreal).await;

    let project = projects::open_matter(
        &surreal,
        &command(
            "Acme Holdings",
            "acme-holdings",
            client_id,
            entity_id,
            acting_person_id,
        ),
    )
    .await
    .expect("open the matter");

    // The code is not echoed into the panic message: CodeQL's
    // rust/cleartext-logging query taints the whole `Project` value through
    // `record_uuid(...)` (used to build `id`/`entity_id`), so interpolating
    // any of its fields into a panic message trips the required CodeQL check
    // (see 738bec0 for the established precedent).
    let suffix = project
        .code
        .strip_prefix("acme-holdings-")
        .unwrap_or_else(|| {
            panic!("expected the stored code to start with the `acme-holdings-` stem")
        });
    // Neither assertion echoes `suffix` itself: it is a substring of
    // `project.code`, which CodeQL's rust/cleartext-logging query taints via
    // `record_uuid(...)` (see the comment above).
    assert_eq!(suffix.len(), 8, "suffix should be exactly 8 characters");
    assert!(
        suffix.bytes().all(|b| b.is_ascii_lowercase()),
        "suffix should be lowercase ASCII letters only"
    );
}

/// The whole point: two matters that would have collided on a hand-picked
/// stem now both open, each with its own generated code.
#[tokio::test]
async fn two_matters_opened_with_the_same_stem_both_succeed_with_distinct_codes() {
    let surreal = mem_surreal().await;
    let (client_id, acting_person_id, entity_id) = references(&surreal).await;

    let first = projects::open_matter(
        &surreal,
        &command(
            "Acme Holdings",
            "acme-holdings",
            client_id,
            entity_id,
            acting_person_id,
        ),
    )
    .await
    .expect("open the first matter");
    let second = projects::open_matter(
        &surreal,
        &command(
            "Acme Holdings II",
            "acme-holdings",
            client_id,
            entity_id,
            acting_person_id,
        ),
    )
    .await
    .expect("open the second matter on the same stem — the generated suffix must disambiguate it");

    assert_ne!(
        first.code, second.code,
        "two matters opened with the same stem must not collide on the stored code"
    );
    assert!(first.code.starts_with("acme-holdings-"));
    assert!(second.code.starts_with("acme-holdings-"));
}

/// The stem is still validated exactly as a hand-typed code always was —
/// generating the suffix does not loosen the shape or reserved-word rules.
#[tokio::test]
async fn a_malformed_or_reserved_stem_is_still_refused() {
    let surreal = mem_surreal().await;
    let (client_id, acting_person_id, entity_id) = references(&surreal).await;

    for bad_stem in ["Not Kebab Case", "new", "navigator", ""] {
        let result = projects::open_matter(
            &surreal,
            &command("A Matter", bad_stem, client_id, entity_id, acting_person_id),
        )
        .await;
        assert!(
            result.is_err(),
            "stem {bad_stem:?} should still be refused, not silently slugified"
        );
    }
}
