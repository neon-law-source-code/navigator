//! Public testimonial reads and the replay-safe canonical seeder seam.

use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::{self, PersonError};
use crate::projects;
use crate::surreal::{record_id, record_uuid, SurrealDb};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedTestimonial {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_name: String,
    pub person_id: Uuid,
    pub person_name: String,
    pub person_title: Option<String>,
    pub profile_image_url: Option<String>,
    pub quote: String,
    pub attribution_label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewTestimonial<'a> {
    pub project_id: Uuid,
    pub person_id: Uuid,
    pub quote: &'a str,
    pub attribution_label: Option<String>,
    pub consented_at: Option<String>,
    pub published_at: Option<String>,
    pub display_order: i32,
}

#[derive(SurrealValue)]
struct TestimonialRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    quote: String,
    attribution_label: Option<String>,
    consented_at: Option<String>,
    published_at: Option<String>,
    display_order: i32,
    inserted_at: String,
    updated_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TestimonialError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("resolve the testimonial senders: {0}")]
    Person(#[from] PersonError),
    #[error(transparent)]
    Project(#[from] projects::ProjectStoreError),
    #[error("writing a testimonial returned no usable row")]
    WriteReturnedNothing,
}

const SELECT: &str = "id, project_id, person_id, quote, attribution_label, consented_at, published_at, display_order, inserted_at, updated_at";

/// Create the testimonial identified by its natural seed key, or return the
/// existing row after a competing canonical seed won its unique index race.
pub async fn find_or_create(
    surreal: &SurrealDb,
    input: &NewTestimonial<'_>,
) -> Result<(), TestimonialError> {
    if find_replay(surreal, input).await?.is_some() {
        return Ok(());
    }
    if projects::find_by_id(surreal, input.project_id)
        .await?
        .is_none()
    {
        return Err(projects::ProjectStoreError::NoSuchProject(input.project_id).into());
    }
    let now = chrono::Utc::now().to_rfc3339();
    let written = surreal
        .query(format!(
            "CREATE $id SET project_id = $project_id, person_id = $person_id, \
             quote = $quote, attribution_label = $attribution_label, consented_at = $consented_at, \
             published_at = $published_at, display_order = $display_order, inserted_at = $now, \
             updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id("testimonial", Uuid::now_v7())))
        .bind(("project_id", record_id("project", input.project_id)))
        .bind(("person_id", record_id("person", input.person_id)))
        .bind(("quote", input.quote.to_string()))
        .bind(("attribution_label", input.attribution_label.clone()))
        .bind(("consented_at", input.consented_at.clone()))
        .bind(("published_at", input.published_at.clone()))
        .bind(("display_order", input.display_order))
        .bind(("now", now))
        .await
        .and_then(surrealdb::IndexedResults::check);
    match written {
        Ok(_) => Ok(()),
        Err(error)
            if crate::surreal::retry::unique_violation(&error) == Some("testimonial_replay") =>
        {
            find_replay(surreal, input)
                .await?
                .ok_or(TestimonialError::WriteReturnedNothing)
                .map(|_| ())
        }
        Err(error) => Err(error.into()),
    }
}

/// Published testimonials for the homepage, ordered exactly as the former SQL
/// read: display order then the lawyer publication timestamp.
pub async fn published_for_home(
    surreal: &SurrealDb,
    limit: u64,
) -> Result<Vec<PublishedTestimonial>, TestimonialError> {
    let query = format!(
        "SELECT {SELECT} FROM testimonial WHERE consented_at != NONE AND published_at != NONE \
         ORDER BY display_order, published_at DESC LIMIT $limit"
    );
    let mut response = surreal
        .query(query)
        .bind(("limit", limit))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<TestimonialRow> = response.take(0)?;
    let project_ids: Vec<Uuid> = rows
        .iter()
        .filter_map(|row| record_uuid(&row.project_id))
        .collect();
    let person_ids: Vec<Uuid> = rows
        .iter()
        .filter_map(|row| record_uuid(&row.person_id))
        .collect();
    let projects: std::collections::HashMap<Uuid, projects::Project> = {
        let mut result = std::collections::HashMap::new();
        for id in project_ids {
            if let Some(project) = projects::find_by_id(surreal, id).await? {
                result.insert(id, project);
            }
        }
        result
    };
    let people: std::collections::HashMap<Uuid, crate::persons::Person> =
        persons::find_by_ids(surreal, &person_ids)
            .await?
            .into_iter()
            .map(|person| (person.id, person))
            .collect();
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let id = record_uuid(&row.id)?;
            let project_id = record_uuid(&row.project_id)?;
            let person_id = record_uuid(&row.person_id)?;
            let project = projects.get(&project_id)?;
            let person = people.get(&person_id)?;
            Some(PublishedTestimonial {
                id,
                project_id,
                project_name: project.name.clone(),
                person_id,
                person_name: person.name.clone(),
                person_title: person.title.clone(),
                profile_image_url: person.profile_image_url.clone(),
                quote: row.quote,
                attribution_label: row.attribution_label,
            })
        })
        .collect())
}

async fn find_replay(
    surreal: &SurrealDb,
    input: &NewTestimonial<'_>,
) -> Result<Option<TestimonialRow>, TestimonialError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM ONLY testimonial WHERE project_id = $project_id \
             AND person_id = $person_id AND quote = $quote LIMIT 1"
        ))
        .bind(("project_id", record_id("project", input.project_id)))
        .bind(("person_id", record_id("person", input.person_id)))
        .bind(("quote", input.quote.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    response.take(0).map_err(TestimonialError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{create, NewProject};
    use crate::test_support::mem_surreal;

    #[tokio::test]
    async fn published_reads_require_consent_and_publication() {
        let surreal = mem_surreal().await;
        let person = persons::create(
            &surreal,
            &crate::persons::NewPerson {
                title: Some("Founder".into()),
                profile_image_url: Some("/images/testimonial.webp".into()),
                ..crate::persons::NewPerson::new("A. Client", "testimonial-port@example.com")
            },
        )
        .await
        .unwrap();
        let project = create(
            &surreal,
            &NewProject {
                code: "testimonial-published".into(),
                name: "Published matter".into(),
                status: "closed".into(),
                entity_id: Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for (quote, published_at, display_order) in [
            ("Published quote.", Some("2026-06-24T00:00:00Z".into()), 1),
            ("Draft quote.", None, 0),
        ] {
            find_or_create(
                &surreal,
                &NewTestimonial {
                    project_id: project.id,
                    person_id: person.id,
                    quote,
                    attribution_label: None,
                    consented_at: Some("2026-06-23T00:00:00Z".into()),
                    published_at,
                    display_order,
                },
            )
            .await
            .unwrap();
        }
        let rows = published_for_home(&surreal, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].quote, "Published quote.");
        assert_eq!(rows[0].project_name, "Published matter");
        assert_eq!(rows[0].person_title.as_deref(), Some("Founder"));
    }
}
