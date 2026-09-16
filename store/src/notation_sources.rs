//! Source links for the information a Notation is assembled from.
//!
//! Email, text messages, and conversations first enter the privileged
//! [`crate::communications`] log. This satellite records only their identity,
//! never a copied message body. Questionnaire sources have no communication
//! row and use their stable questionnaire reference instead.

use uuid::Uuid;

use crate::surreal::{record_id, SurrealDb};

const TABLE: &str = "notation_source";

/// Kinds of intake Navigator can trace into a Notation.
pub mod kind {
    pub const EMAIL: &str = "email";
    pub const TEXT_MESSAGE: &str = "text_message";
    pub const CONVERSATION: &str = "conversation";
    pub const QUESTIONNAIRE: &str = "questionnaire";
}

/// Link one source to a notation in lawyer-review order.
#[derive(Debug, Clone)]
pub struct NewNotationSource {
    pub notation_id: Uuid,
    pub kind: String,
    pub communication_id: Option<Uuid>,
    /// A question code for questionnaire facts or an external source id.
    pub source_ref: Option<String>,
    pub position: i32,
}

/// Persist a source link without copying privileged intake content.
///
/// # Errors
///
/// Propagates database failures, including duplicate review positions.
pub async fn attach(db: &SurrealDb, source: &NewNotationSource) -> Result<(), surrealdb::Error> {
    let id = Uuid::now_v7();
    db.query(
        "CREATE $id SET notation_id = $notation_id, kind = $kind, \
         communication_id = $communication_id, source_ref = $source_ref, position = $position",
    )
    .bind(("id", record_id(TABLE, id)))
    .bind(("notation_id", record_id("notation", source.notation_id)))
    .bind(("kind", source.kind.clone()))
    .bind((
        "communication_id",
        source
            .communication_id
            .map(|id| record_id("communication", id)),
    ))
    .bind(("source_ref", source.source_ref.clone()))
    .bind(("position", source.position))
    .await
    .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{attach, kind, NewNotationSource};
    use crate::surreal::test_support::mem;

    #[tokio::test]
    async fn questionnaire_sources_do_not_need_a_duplicated_communication() {
        let db = mem().await;
        let notation_id = crate::test_support::seed_notation(&db).await;
        attach(
            &db,
            &NewNotationSource {
                notation_id,
                kind: kind::QUESTIONNAIRE.into(),
                communication_id: None,
                source_ref: Some("family__beneficiaries".into()),
                position: 0,
            },
        )
        .await
        .unwrap();
    }
}
