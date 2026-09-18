//! Public testimonial reads and the replay-safe canonical seeder seam.

use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::{self, PersonError};
use crate::projects;
use crate::surreal::{record_id, record_uuid, SurrealDb};

/// A testimonial row returned to an authenticated matter surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Testimonial {
    pub id: Uuid,
    pub project_id: Uuid,
    pub person_id: Uuid,
    pub quote: String,
    pub attribution_label: Option<String>,
    pub consented_at: Option<String>,
    pub published_at: Option<String>,
    pub display_order: i32,
}

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

impl TestimonialRow {
    fn into_testimonial(self) -> Option<Testimonial> {
        Some(Testimonial {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            person_id: record_uuid(&self.person_id)?,
            quote: self.quote,
            attribution_label: self.attribution_label,
            consented_at: self.consented_at,
            published_at: self.published_at,
            display_order: self.display_order,
        })
    }
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
    #[error("the caller is not authorized to change this testimonial")]
    NotAuthorized,
    #[error("testimonial not found")]
    NotFound,
    #[error("a testimonial must have public consent before publication")]
    NotConsented,
}

const SELECT: &str = "id, project_id, person_id, quote, attribution_label, consented_at, published_at, display_order, inserted_at, updated_at";

/// The client-side fields submitted by the accountable client. The project and
/// person are deliberately not part of this input: both come from the route
/// and authenticated session, respectively.
#[derive(Clone, Debug)]
pub struct TestimonialSubmission<'a> {
    pub quote: &'a str,
    pub attribution_label: Option<String>,
    pub request_public: bool,
}

/// Read the current testimonial for one person on one project.
pub async fn for_person_project(
    surreal: &SurrealDb,
    person_id: Uuid,
    project_id: Uuid,
) -> Result<Option<Testimonial>, TestimonialError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM testimonial WHERE project_id = $project_id \
             AND person_id = $person_id ORDER BY updated_at DESC LIMIT 1"
        ))
        .bind(("project_id", record_id("project", project_id)))
        .bind(("person_id", record_id("person", person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<TestimonialRow> = response.take(0)?;
    Ok(row.and_then(TestimonialRow::into_testimonial))
}

/// Create or edit the testimonial owned by the project's client DRI.
///
/// The participation row is the authorization boundary. A client request can
/// grant consent, but it can never publish: every client write clears
/// `published_at`, requiring a fresh lawyer/admin approval after an edit.
pub async fn save_for_client_dri(
    surreal: &SurrealDb,
    person_id: Uuid,
    project_id: Uuid,
    input: &TestimonialSubmission<'_>,
) -> Result<Testimonial, TestimonialError> {
    let Some(participation) =
        projects::participation_for_person(surreal, person_id, project_id).await?
    else {
        return Err(TestimonialError::NotAuthorized);
    };
    if !participation.is_client_dri
        || !projects::PARTICIPATION_CLIENT_SIDE.contains(&participation.participation.as_str())
    {
        return Err(TestimonialError::NotAuthorized);
    }
    let quote = input.quote.trim();
    if quote.is_empty() {
        return Err(TestimonialError::WriteReturnedNothing);
    }
    let attribution_label = input
        .attribution_label
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let existing = for_person_project(surreal, person_id, project_id).await?;
    let id = existing
        .as_ref()
        .map_or_else(Uuid::now_v7, |testimonial| testimonial.id);
    let now = chrono::Utc::now().to_rfc3339();
    let consented_at = input.request_public.then(|| now.clone());
    let query = if existing.is_some() {
        format!(
            "UPDATE $id SET quote = $quote, attribution_label = $attribution_label, \
             consented_at = $consented_at, published_at = NONE, updated_at = $now RETURN {SELECT}"
        )
    } else {
        format!(
            "CREATE $id SET project_id = $project_id, person_id = $person_id, quote = $quote, \
             attribution_label = $attribution_label, consented_at = $consented_at, \
             published_at = NONE, display_order = 0, inserted_at = $now, updated_at = $now RETURN {SELECT}"
        )
    };
    let mut response = surreal
        .query(query)
        .bind(("id", record_id("testimonial", id)))
        .bind(("project_id", record_id("project", project_id)))
        .bind(("person_id", record_id("person", person_id)))
        .bind(("quote", quote.to_string()))
        .bind(("attribution_label", attribution_label))
        .bind(("consented_at", consented_at))
        .bind(("now", now))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    response
        .take::<Option<TestimonialRow>>(0)?
        .and_then(TestimonialRow::into_testimonial)
        .ok_or(TestimonialError::WriteReturnedNothing)
}

async fn firm_actor_can_publish(
    surreal: &SurrealDb,
    actor_person_id: Option<Uuid>,
    actor_role: persons::Role,
    project_id: Uuid,
) -> Result<bool, TestimonialError> {
    if !actor_role.is_lawyer_tier() {
        return Ok(false);
    }
    let Some(actor_person_id) = actor_person_id else {
        return Ok(false);
    };
    Ok(
        projects::participation_for_person(surreal, actor_person_id, project_id)
            .await?
            .is_some_and(|row| {
                !projects::PARTICIPATION_CLIENT_SIDE.contains(&row.participation.as_str())
            }),
    )
}

/// Approve a consented testimonial for the public homepage.
pub async fn publish(
    surreal: &SurrealDb,
    actor_person_id: Option<Uuid>,
    actor_role: persons::Role,
    testimonial_id: Uuid,
) -> Result<Testimonial, TestimonialError> {
    let Some(current) = find_by_id(surreal, testimonial_id).await? else {
        return Err(TestimonialError::NotFound);
    };
    if !firm_actor_can_publish(surreal, actor_person_id, actor_role, current.project_id).await? {
        return Err(TestimonialError::NotAuthorized);
    }
    if current.consented_at.is_none() {
        return Err(TestimonialError::NotConsented);
    }
    update_publication(
        surreal,
        testimonial_id,
        Some(chrono::Utc::now().to_rfc3339()),
    )
    .await
}

/// Remove a testimonial from the public homepage while preserving the client's
/// consent record for a later re-approval.
pub async fn unpublish(
    surreal: &SurrealDb,
    actor_person_id: Option<Uuid>,
    actor_role: persons::Role,
    testimonial_id: Uuid,
) -> Result<Testimonial, TestimonialError> {
    let Some(current) = find_by_id(surreal, testimonial_id).await? else {
        return Err(TestimonialError::NotFound);
    };
    if !firm_actor_can_publish(surreal, actor_person_id, actor_role, current.project_id).await? {
        return Err(TestimonialError::NotAuthorized);
    }
    update_publication(surreal, testimonial_id, None).await
}

async fn find_by_id(
    surreal: &SurrealDb,
    testimonial_id: Uuid,
) -> Result<Option<Testimonial>, TestimonialError> {
    let mut response = surreal
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id("testimonial", testimonial_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<TestimonialRow> = response.take(0)?;
    Ok(row.and_then(TestimonialRow::into_testimonial))
}

async fn update_publication(
    surreal: &SurrealDb,
    testimonial_id: Uuid,
    published_at: Option<String>,
) -> Result<Testimonial, TestimonialError> {
    let mut response = surreal
        .query(format!(
            "UPDATE $id SET published_at = $published_at, updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id("testimonial", testimonial_id)))
        .bind(("published_at", published_at))
        .bind(("now", chrono::Utc::now().to_rfc3339()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    response
        .take::<Option<TestimonialRow>>(0)?
        .and_then(TestimonialRow::into_testimonial)
        .ok_or(TestimonialError::WriteReturnedNothing)
}

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
    use crate::projects::{create, designate_dri_in_surreal, DriSide, NewProject};
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

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn only_the_client_dri_can_submit_and_publication_is_two_step() {
        let surreal = mem_surreal().await;
        let client = persons::create(
            &surreal,
            &crate::persons::NewPerson::new("Synthetic Client", "testimonial-client@example.com"),
        )
        .await
        .unwrap();
        let other_client = persons::create(
            &surreal,
            &crate::persons::NewPerson::new("Synthetic Other", "testimonial-other@example.com"),
        )
        .await
        .unwrap();
        let lawyer = persons::create(
            &surreal,
            &crate::persons::NewPerson {
                role: persons::Role::Lawyer,
                ..crate::persons::NewPerson::new(
                    "Synthetic Lawyer",
                    "testimonial-lawyer@example.com",
                )
            },
        )
        .await
        .unwrap();
        let project = create(
            &surreal,
            &NewProject {
                code: "testimonial-workflow".into(),
                name: "Synthetic testimonial matter".into(),
                status: "open".into(),
                entity_id: Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        designate_dri_in_surreal(&surreal, project.id, client.id, DriSide::Client)
            .await
            .unwrap();
        designate_dri_in_surreal(&surreal, project.id, lawyer.id, DriSide::Lawyer)
            .await
            .unwrap();

        let private = save_for_client_dri(
            &surreal,
            client.id,
            project.id,
            &TestimonialSubmission {
                quote: "A private note.",
                attribution_label: Some("Founder".into()),
                request_public: false,
            },
        )
        .await
        .unwrap();
        assert!(private.consented_at.is_none());
        assert!(private.published_at.is_none());
        assert!(published_for_home(&surreal, 10).await.unwrap().is_empty());
        assert!(matches!(
            save_for_client_dri(
                &surreal,
                other_client.id,
                project.id,
                &TestimonialSubmission {
                    quote: "Not mine.",
                    attribution_label: None,
                    request_public: true,
                },
            )
            .await,
            Err(TestimonialError::NotAuthorized)
        ));

        let requested = save_for_client_dri(
            &surreal,
            client.id,
            project.id,
            &TestimonialSubmission {
                quote: "A public request.",
                attribution_label: Some("Founder".into()),
                request_public: true,
            },
        )
        .await
        .unwrap();
        assert!(requested.consented_at.is_some());
        assert!(requested.published_at.is_none());
        assert!(published_for_home(&surreal, 10).await.unwrap().is_empty());

        let published = publish(
            &surreal,
            Some(lawyer.id),
            persons::Role::Lawyer,
            requested.id,
        )
        .await
        .unwrap();
        assert!(published.published_at.is_some());
        assert_eq!(published_for_home(&surreal, 10).await.unwrap().len(), 1);

        let edited = save_for_client_dri(
            &surreal,
            client.id,
            project.id,
            &TestimonialSubmission {
                quote: "An edited public request.",
                attribution_label: Some("Founder".into()),
                request_public: true,
            },
        )
        .await
        .unwrap();
        assert!(edited.consented_at.is_some());
        assert!(edited.published_at.is_none());
        assert!(published_for_home(&surreal, 10).await.unwrap().is_empty());

        publish(&surreal, Some(lawyer.id), persons::Role::Lawyer, edited.id)
            .await
            .unwrap();

        let unpublished = unpublish(
            &surreal,
            Some(lawyer.id),
            persons::Role::Lawyer,
            requested.id,
        )
        .await
        .unwrap();
        assert!(unpublished.consented_at.is_some());
        assert!(unpublished.published_at.is_none());
        assert!(published_for_home(&surreal, 10).await.unwrap().is_empty());
    }
}
