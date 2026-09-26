//! The firm glossary as reference data (#894), on the engine that holds
//! the table since its ENG-20 slice.
//!
//! The load-bearing test here is the last one. A contract's **defined
//! terms** are matter data — per-project, never seeded, covered by the
//! no-client-data rule — and the **firm glossary** is universal reference
//! data. If they ever shared a table, firm vocabulary would be polluted
//! with client content and unwinding it would be very hard. So this suite
//! does not merely assert that no matter-scoped rows are present today;
//! it introspects the applied schema and asserts the table has no field
//! one could be written into.

use store::glossary;
use store::surreal::SurrealDb;

async fn mem() -> SurrealDb {
    store::test_support::mem_surreal().await
}

/// The canonical seed materializes the authored vocabulary, and running it
/// again converges instead of appending — an edited definition updates its
/// row rather than creating a rival one under the same slug.
#[tokio::test]
async fn materialize_is_idempotent_and_updates_in_place() {
    let db = mem().await;

    let written = glossary::materialize(&db, glossary::terms())
        .await
        .expect("first materialize");
    assert!(written > 25, "expected the full vocabulary, got {written}");
    let after_first = glossary::all(&db).await.expect("all").len();
    assert_eq!(after_first, written);

    glossary::materialize(&db, glossary::terms())
        .await
        .expect("second materialize");
    assert_eq!(
        glossary::all(&db).await.expect("all").len(),
        after_first,
        "a re-run must converge, not append a second row per term"
    );

    // An edited definition updates the existing row.
    let edited = glossary::Term {
        slug: "lawyer-review".to_string(),
        title: "Lawyer Review".to_string(),
        description: "A lawyer reviews the work.".to_string(),
        body: "Rewritten body.".to_string(),
    };
    glossary::materialize(&db, &[edited]).await.expect("edit");
    let row = glossary::by_slug(&db, "lawyer-review")
        .await
        .expect("lookup")
        .expect("lawyer-review row");
    assert_eq!(row.body, "Rewritten body.");
    assert_eq!(
        glossary::all(&db).await.expect("all").len(),
        after_first,
        "editing a term must not create a rival row under the same slug"
    );
}

/// The reference a composition performs: a dashboard section, notation
/// template, or questionnaire prompt names a term by slug instead of
/// restating the definition, and that slug resolves.
#[tokio::test]
async fn a_term_referenced_by_slug_resolves_to_its_definition() {
    let db = mem().await;
    glossary::materialize(&db, glossary::terms())
        .await
        .expect("materialize");

    let referenced = glossary::by_slug(&db, "lawyer-review")
        .await
        .expect("lookup")
        .expect("a composition referencing `lawyer-review` must resolve");
    assert_eq!(referenced.title, "Lawyer Review");
    assert!(
        !referenced.body.trim().is_empty(),
        "a resolved reference must carry the definition, not just a title"
    );

    // #894 also adds Template, absent until now despite #850 existing to
    // document its four-part anatomy.
    let template = glossary::by_slug(&db, "template")
        .await
        .expect("lookup")
        .expect("Template must be a real term");
    assert!(
        template.body.contains("not a literal nested YAML key"),
        "the metadata caveat is the load-bearing half of the Template entry"
    );

    assert!(
        glossary::by_slug(&db, "not-a-real-term")
            .await
            .expect("lookup")
            .is_none(),
        "an unknown slug resolves to nothing rather than a wrong definition"
    );
}

/// **A contract's defined terms must never share this table.** Asserting
/// "no matter-scoped rows exist" would only describe today's data; this
/// introspects the applied schema, so a later schema edit cannot quietly
/// add the field that would make the mixture possible.
#[tokio::test]
async fn the_glossary_table_cannot_hold_matter_scoped_rows() {
    let db = mem().await;

    let introspection = store::schema::introspect(&db).await.expect("introspect");
    let table = introspection
        .get("glossary_term")
        .expect("the applied schema defines glossary_term");

    let mut fields: Vec<&str> = table.fields.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec!["body", "inserted_at", "slug", "title", "updated_at"],
        "glossary_term carries firm vocabulary and nothing else"
    );
    for scoped in ["project_id", "person_id", "entity_id", "notation_id"] {
        assert!(
            !table.fields.contains_key(scoped),
            "`{scoped}` would let a contract's defined term be filed as firm vocabulary"
        );
    }

    // Nor may it reach a matter-scoped table by reference: a `record<T>`
    // field is Surreal's foreign key, and this table must define none.
    for (name, definition) in &table.fields {
        assert!(
            !definition.contains("record<"),
            "field `{name}` links to another table: {definition}"
        );
    }
}
