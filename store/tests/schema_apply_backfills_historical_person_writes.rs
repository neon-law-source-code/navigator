//! The historical person-flag backfill is load-bearing on the sign-in
//! *write*, not the typed read.
//!
//! A Person created before `email_confirmed` and `is_admitted` existed still
//! reads through `PersonRow` and `is_admitted`: both already treat a missing
//! value as the write-time default. Sign-in then calls `link_oidc_subject`,
//! which issues a full-record `UPDATE`. Surreal validates every defined
//! field on that write, so a missing required bool fails with `Expected
//! bool but found NONE` until `apply` materializes the defaults.

use store::persons;
use store::schema;
use store::surreal::test_support::unmigrated;
use uuid::Uuid;

const HISTORICAL_EMAIL: &str = "historical.person@example.com";
const PRESERVED_EMAIL: &str = "preserved.flags@example.com";
const NEW_SUBJECT: &str = "historical-sign-in-subject";

#[tokio::test]
async fn applying_lets_sign_in_link_an_oidc_subject_on_a_historical_person() {
    let db = unmigrated().await;
    let historical_id = Uuid::now_v7();
    let preserved_id = Uuid::now_v7();

    db.query(
        "CREATE $id SET name = 'Historical Person', \
         email = $email, email_lower = $email_lower, \
         role = 'client', \
         inserted_at = type::datetime('2020-01-01T00:00:00Z'), \
         updated_at = type::datetime('2020-01-01T00:00:00Z')",
    )
    .bind(("id", store::surreal::record_id("person", historical_id)))
    .bind(("email", HISTORICAL_EMAIL))
    .bind(("email_lower", HISTORICAL_EMAIL))
    .await
    .unwrap()
    .check()
    .unwrap();

    db.query(
        "CREATE $id SET name = 'Preserved Flags', \
         email = $email, email_lower = $email_lower, \
         role = 'client', email_confirmed = true, is_admitted = false, \
         inserted_at = type::datetime('2020-01-01T00:00:00Z'), \
         updated_at = type::datetime('2020-01-01T00:00:00Z')",
    )
    .bind(("id", store::surreal::record_id("person", preserved_id)))
    .bind(("email", PRESERVED_EMAIL))
    .bind(("email_lower", PRESERVED_EMAIL))
    .await
    .unwrap()
    .check()
    .unwrap();

    schema::apply(&db)
        .await
        .expect("apply converges definitions and materializes missing person flags");

    assert!(
        persons::find_by_oidc_subject(&db, NEW_SUBJECT)
            .await
            .unwrap()
            .is_none(),
        "a second provider's subject is not already linked"
    );

    let resolved = persons::find_by_email_ci(&db, HISTORICAL_EMAIL)
        .await
        .unwrap()
        .expect("a historical mailbox still resolves after apply");
    assert_eq!(resolved.id, historical_id);
    assert!(resolved.oidc_subject.is_none());
    assert!(!resolved.email_confirmed);

    let linked = persons::link_oidc_subject(&db, resolved.id, NEW_SUBJECT)
        .await
        .expect("linking a subject must succeed after the person-flag backfill")
        .expect("the historical person still exists after the link");
    assert_eq!(linked.id, historical_id);
    assert_eq!(linked.oidc_subject.as_deref(), Some(NEW_SUBJECT));
    assert!(!linked.email_confirmed);

    let persisted = persons::find_by_oidc_subject(&db, NEW_SUBJECT)
        .await
        .unwrap()
        .expect("the linked subject must persist");
    assert_eq!(persisted.id, historical_id);
    assert_eq!(persisted.oidc_subject.as_deref(), Some(NEW_SUBJECT));
    assert!(!persisted.email_confirmed);
    assert!(persons::is_admitted(&db, historical_id).await.unwrap());

    let preserved = persons::find_by_email_ci(&db, PRESERVED_EMAIL)
        .await
        .unwrap()
        .expect("an already-flagged person remains readable");
    assert_eq!(preserved.id, preserved_id);
    assert!(preserved.email_confirmed);
    assert!(!persons::is_admitted(&db, preserved_id).await.unwrap());

    let confirmed: Vec<bool> = db
        .query("SELECT VALUE email_confirmed FROM ONLY $id")
        .bind(("id", store::surreal::record_id("person", preserved_id)))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(confirmed, vec![true]);
    let admitted: Vec<bool> = db
        .query("SELECT VALUE is_admitted FROM ONLY $id")
        .bind(("id", store::surreal::record_id("person", preserved_id)))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(admitted, vec![false]);

    assert_reapply_preserves_person_defaults(&db, historical_id, preserved_id).await;
}

async fn assert_reapply_preserves_person_defaults(
    db: &store::surreal::SurrealDb,
    historical_id: Uuid,
    preserved_id: Uuid,
) {
    schema::apply(db)
        .await
        .expect("a second apply skips the converged person-default backfill");

    let linked_after_reapply = persons::find_by_oidc_subject(db, NEW_SUBJECT)
        .await
        .unwrap()
        .expect("the linked subject remains after reapplying the schema");
    assert_eq!(linked_after_reapply.id, historical_id);
    assert_eq!(
        linked_after_reapply.oidc_subject.as_deref(),
        Some(NEW_SUBJECT)
    );
    assert!(!linked_after_reapply.email_confirmed);
    assert!(persons::is_admitted(db, historical_id).await.unwrap());

    let preserved_after_reapply = persons::find_by_email_ci(db, PRESERVED_EMAIL)
        .await
        .unwrap()
        .expect("the preserved person remains after reapplying the schema");
    assert_eq!(preserved_after_reapply.id, preserved_id);
    assert!(preserved_after_reapply.email_confirmed);
    assert!(!persons::is_admitted(db, preserved_id).await.unwrap());
}
