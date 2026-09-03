//! `open_matter` stores the caller's supplied code exactly, never a
//! generated one.
//!
//! A hand-picked code can never change once chosen (`project.code` is
//! `READONLY`), and it is a coordinate shared with three systems Navigator
//! does not own: a repository's `navigator.yaml`, the matter's Drive folder
//! name, and its Notion `Project code` URL. Those are set independently of
//! Navigator and never renamed to follow it, so `open_matter` generating a
//! different code than the one the caller supplied would strand every one of
//! those bindings the moment the row is written. The uniqueness guarantee
//! comes from a rejected collision (`OpenMatterError::CodeConflict`), not
//! from generation.

use store::persons::{self, NewPerson, Role};
use store::projects::{self, OpenMatterCommand, OpenMatterError};
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
        brand: "neon".to_string(),
        attestation: true,
        acting_person_id,
    }
}

/// The stored code is the supplied code, verbatim — no suffix, no other
/// transformation.
#[tokio::test]
async fn open_matter_stores_the_supplied_code_verbatim() {
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

    assert_eq!(project.code, "acme-holdings");
}

/// A code needing normalization is stored lowercased — the only
/// transformation `open_matter` still applies.
#[tokio::test]
async fn a_code_needing_normalization_is_stored_lowercased() {
    let surreal = mem_surreal().await;
    let (client_id, acting_person_id, entity_id) = references(&surreal).await;

    let project = projects::open_matter(
        &surreal,
        &command("Litbox", "Litbox", client_id, entity_id, acting_person_id),
    )
    .await
    .expect("open the matter");

    assert_eq!(project.code, "litbox");
}

/// Two matters cannot open on the same code: the caller owns this
/// coordinate, so a collision is refused rather than silently disambiguated,
/// and the first matter's row is unaffected.
#[tokio::test]
async fn a_second_matter_on_the_same_code_is_refused_with_a_conflict() {
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
    .await;

    // `second` is not echoed into the assertion message on failure: an `Ok`
    // value would carry a `Project`, and CodeQL's rust/cleartext-logging
    // query taints that whole struct through `record_uuid(...)` (used to
    // build `id`/`entity_id`), so interpolating it into a panic message trips
    // the required CodeQL check (see 738bec0 for the established precedent).
    assert!(
        matches!(second, Err(OpenMatterError::CodeConflict)),
        "a second open on the same code must be refused as a conflict, not disambiguated"
    );

    let reloaded = projects::find_by_id(&surreal, first.id)
        .await
        .expect("reload the first matter")
        .expect("the first matter still exists");
    assert_eq!(
        reloaded.code, "acme-holdings",
        "the first matter's code must be unchanged by the rejected second open"
    );
}

/// The code is still validated exactly as a hand-typed code always was —
/// storing it verbatim does not loosen the shape or reserved-word rules.
#[tokio::test]
async fn a_malformed_or_reserved_code_is_still_refused() {
    let surreal = mem_surreal().await;
    let (client_id, acting_person_id, entity_id) = references(&surreal).await;

    for bad_code in ["Not Kebab Case", "new", "navigator", ""] {
        let result = projects::open_matter(
            &surreal,
            &command("A Matter", bad_code, client_id, entity_id, acting_person_id),
        )
        .await;
        assert!(
            result.is_err(),
            "code {bad_code:?} should still be refused, not silently slugified"
        );
    }
}
