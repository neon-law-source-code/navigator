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

/// The homepage projection of a published testimonial. Identity stays on the
/// private row: public output is the quote plus the client's chosen
/// attribution, and nothing inferred from the Person record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedTestimonial {
    pub id: Uuid,
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
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(PublishedTestimonial {
                id: record_uuid(&row.id)?,
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
        let public = format!("{:?}", rows[0]);
        assert!(
            !public.contains("Published matter"),
            "public testimonial data must not carry its matter title"
        );
        assert!(
            !public.contains("A. Client"),
            "public projection must not carry the stored Person name: {public}"
        );
        assert!(
            !public.contains("Founder"),
            "public projection must not fall back to the stored Person title: {public}"
        );
        assert!(
            !public.contains("/images/testimonial.webp"),
            "public projection must not carry a profile image: {public}"
        );
        assert!(rows[0].attribution_label.is_none());
    }

    #[tokio::test]
    async fn public_projection_uses_chosen_attribution_and_omits_blank_identity() {
        let surreal = mem_surreal().await;
        let person = persons::create(
            &surreal,
            &crate::persons::NewPerson {
                title: Some("Stored Title".into()),
                profile_image_url: Some("/images/stored-identity.webp".into()),
                ..crate::persons::NewPerson::new(
                    "Stored Identity",
                    "testimonial-chosen@example.com",
                )
            },
        )
        .await
        .unwrap();
        let project = create(
            &surreal,
            &NewProject {
                code: "testimonial-chosen".into(),
                name: "Chosen attribution matter".into(),
                status: "closed".into(),
                entity_id: Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        find_or_create(
            &surreal,
            &NewTestimonial {
                project_id: project.id,
                person_id: person.id,
                quote: "Use the label I chose.",
                attribution_label: Some("Chosen Label".into()),
                consented_at: Some("2026-06-23T00:00:00Z".into()),
                published_at: Some("2026-06-24T00:00:00Z".into()),
                display_order: 1,
            },
        )
        .await
        .unwrap();
        find_or_create(
            &surreal,
            &NewTestimonial {
                project_id: project.id,
                person_id: person.id,
                quote: "Publish the quote without a name.",
                attribution_label: None,
                consented_at: Some("2026-06-23T00:00:00Z".into()),
                published_at: Some("2026-06-25T00:00:00Z".into()),
                display_order: 2,
            },
        )
        .await
        .unwrap();
        let rows = published_for_home(&surreal, 10).await.unwrap();
        assert_eq!(rows.len(), 2);
        let chosen = rows
            .iter()
            .find(|row| row.quote == "Use the label I chose.")
            .expect("chosen attribution row");
        assert_eq!(chosen.attribution_label.as_deref(), Some("Chosen Label"));
        let blank = rows
            .iter()
            .find(|row| row.quote == "Publish the quote without a name.")
            .expect("blank attribution row");
        assert!(blank.attribution_label.is_none());
        let public = format!("{rows:?}");
        for leaked in [
            "Stored Identity",
            "Stored Title",
            "/images/stored-identity.webp",
        ] {
            assert!(
                !public.contains(leaked),
                "public projection leaked `{leaked}`: {public}"
            );
        }
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

    struct BoundaryFixture {
        surreal: SurrealDb,
        client: persons::Person,
        other_client: persons::Person,
        lawyer: persons::Person,
        clerk: persons::Person,
        admin: persons::Person,
        owner: persons::Person,
        project: projects::Project,
        other_project: projects::Project,
    }

    async fn person_with_role(
        surreal: &SurrealDb,
        name: &str,
        email: &str,
        role: persons::Role,
    ) -> persons::Person {
        persons::create(
            surreal,
            &crate::persons::NewPerson {
                role,
                ..crate::persons::NewPerson::new(name, email)
            },
        )
        .await
        .unwrap()
    }

    async fn matter(surreal: &SurrealDb, code: &str, name: &str) -> projects::Project {
        create(
            surreal,
            &NewProject {
                code: code.into(),
                name: name.into(),
                status: "open".into(),
                entity_id: Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }

    async fn boundary_fixture() -> BoundaryFixture {
        let surreal = mem_surreal().await;
        let client = person_with_role(
            &surreal,
            "Boundary Client",
            "testimonial-boundary-client@example.com",
            persons::Role::Client,
        )
        .await;
        let other_client = person_with_role(
            &surreal,
            "Boundary Other Client",
            "testimonial-boundary-other@example.com",
            persons::Role::Client,
        )
        .await;
        let lawyer = person_with_role(
            &surreal,
            "Boundary Lawyer",
            "testimonial-boundary-lawyer@example.com",
            persons::Role::Lawyer,
        )
        .await;
        let clerk = person_with_role(
            &surreal,
            "Boundary Clerk",
            "testimonial-boundary-clerk@example.com",
            persons::Role::Clerk,
        )
        .await;
        let admin = person_with_role(
            &surreal,
            "Boundary Admin",
            "testimonial-boundary-admin@example.com",
            persons::Role::Admin,
        )
        .await;
        let owner = person_with_role(
            &surreal,
            "Boundary Owner",
            "testimonial-boundary-owner@example.com",
            persons::Role::Owner,
        )
        .await;
        let project = matter(&surreal, "testimonial-boundary", "Boundary matter").await;
        let other_project = matter(
            &surreal,
            "testimonial-boundary-other",
            "Other boundary matter",
        )
        .await;
        designate_dri_in_surreal(&surreal, project.id, client.id, DriSide::Client)
            .await
            .unwrap();
        designate_dri_in_surreal(&surreal, project.id, lawyer.id, DriSide::Lawyer)
            .await
            .unwrap();
        designate_dri_in_surreal(&surreal, other_project.id, other_client.id, DriSide::Client)
            .await
            .unwrap();
        designate_dri_in_surreal(&surreal, other_project.id, lawyer.id, DriSide::Lawyer)
            .await
            .unwrap();
        for (person_id, participation) in [
            (clerk.id, "clerk"),
            (admin.id, "admin"),
            (owner.id, "owner"),
        ] {
            projects::add_participation(&surreal, project.id, person_id, participation)
                .await
                .unwrap();
        }
        BoundaryFixture {
            surreal,
            client,
            other_client,
            lawyer,
            clerk,
            admin,
            owner,
            project,
            other_project,
        }
    }

    fn public_request(quote: &str) -> TestimonialSubmission<'_> {
        TestimonialSubmission {
            quote,
            attribution_label: Some("Founder".into()),
            request_public: true,
        }
    }

    fn private_request(quote: &str) -> TestimonialSubmission<'_> {
        TestimonialSubmission {
            quote,
            attribution_label: Some("Founder".into()),
            request_public: false,
        }
    }

    async fn snapshot(
        surreal: &SurrealDb,
        person_id: Uuid,
        project_id: Uuid,
    ) -> Option<Testimonial> {
        for_person_project(surreal, person_id, project_id)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_client_cannot_submit_for_the_wrong_project() {
        let fixture = boundary_fixture().await;
        let saved = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &public_request("Kept on the right matter."),
        )
        .await
        .unwrap();
        let before = snapshot(&fixture.surreal, fixture.client.id, fixture.project.id).await;

        assert!(matches!(
            save_for_client_dri(
                &fixture.surreal,
                fixture.client.id,
                fixture.other_project.id,
                &public_request("Wrong matter."),
            )
            .await,
            Err(TestimonialError::NotAuthorized)
        ));
        assert_eq!(
            snapshot(&fixture.surreal, fixture.client.id, fixture.project.id).await,
            before
        );
        assert!(snapshot(
            &fixture.surreal,
            fixture.client.id,
            fixture.other_project.id
        )
        .await
        .is_none());
        assert!(snapshot(
            &fixture.surreal,
            fixture.other_client.id,
            fixture.other_project.id
        )
        .await
        .is_none());
        assert_eq!(saved.quote, "Kept on the right matter.");
    }

    #[tokio::test]
    async fn non_client_tiers_cannot_create_client_consent() {
        let fixture = boundary_fixture().await;
        for (person_id, role) in [
            (fixture.lawyer.id, persons::Role::Lawyer),
            (fixture.clerk.id, persons::Role::Clerk),
            (fixture.admin.id, persons::Role::Admin),
            (fixture.owner.id, persons::Role::Owner),
        ] {
            assert!(
                matches!(
                    save_for_client_dri(
                        &fixture.surreal,
                        person_id,
                        fixture.project.id,
                        &public_request("Firm-written consent."),
                    )
                    .await,
                    Err(TestimonialError::NotAuthorized)
                ),
                "{role:?} must not write client consent"
            );
            assert!(
                snapshot(&fixture.surreal, person_id, fixture.project.id)
                    .await
                    .is_none(),
                "{role:?} refusal must not insert a row"
            );
        }
        assert!(
            snapshot(&fixture.surreal, fixture.client.id, fixture.project.id)
                .await
                .is_none()
        );
        assert!(published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn admin_cannot_manufacture_or_overwrite_client_consent() {
        let fixture = boundary_fixture().await;
        let consented = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &public_request("Client consent."),
        )
        .await
        .unwrap();
        let published = publish(
            &fixture.surreal,
            Some(fixture.lawyer.id),
            persons::Role::Lawyer,
            consented.id,
        )
        .await
        .unwrap();
        let before = snapshot(&fixture.surreal, fixture.client.id, fixture.project.id).await;

        assert!(matches!(
            save_for_client_dri(
                &fixture.surreal,
                fixture.admin.id,
                fixture.project.id,
                &public_request("Admin overwrite."),
            )
            .await,
            Err(TestimonialError::NotAuthorized)
        ));
        let after = snapshot(&fixture.surreal, fixture.client.id, fixture.project.id).await;
        assert_eq!(after, before);
        assert_eq!(
            after.as_ref().map(|row| row.quote.as_str()),
            Some("Client consent.")
        );
        assert_eq!(
            after.as_ref().and_then(|row| row.consented_at.as_deref()),
            published.consented_at.as_deref()
        );
        assert!(after.as_ref().is_some_and(|row| row.published_at.is_some()));
        assert_eq!(
            published_for_home(&fixture.surreal, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_client_cannot_publish_directly() {
        let fixture = boundary_fixture().await;
        let saved = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &public_request("Waiting for approval."),
        )
        .await
        .unwrap();
        assert!(saved.published_at.is_none());
        let before = snapshot(&fixture.surreal, fixture.client.id, fixture.project.id).await;

        assert!(matches!(
            publish(
                &fixture.surreal,
                Some(fixture.client.id),
                persons::Role::Client,
                saved.id,
            )
            .await,
            Err(TestimonialError::NotAuthorized)
        ));
        assert_eq!(
            snapshot(&fixture.surreal, fixture.client.id, fixture.project.id).await,
            before
        );
        assert!(published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn revoking_consent_removes_a_published_testimonial_from_public_reads() {
        let fixture = boundary_fixture().await;
        let saved = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &public_request("Publish me."),
        )
        .await
        .unwrap();
        publish(
            &fixture.surreal,
            Some(fixture.lawyer.id),
            persons::Role::Lawyer,
            saved.id,
        )
        .await
        .unwrap();
        assert_eq!(
            published_for_home(&fixture.surreal, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        let revoked = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &private_request("Keep this private now."),
        )
        .await
        .unwrap();
        assert!(revoked.consented_at.is_none());
        assert!(revoked.published_at.is_none());
        assert!(published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn editing_clears_publication_and_follows_the_submitted_consent_choice() {
        let fixture = boundary_fixture().await;
        let first = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &public_request("First public request."),
        )
        .await
        .unwrap();
        publish(
            &fixture.surreal,
            Some(fixture.lawyer.id),
            persons::Role::Lawyer,
            first.id,
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let renewed = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &public_request("Edited public request."),
        )
        .await
        .unwrap();
        assert_eq!(renewed.quote, "Edited public request.");
        assert!(renewed.published_at.is_none());
        assert!(renewed.consented_at.is_some());
        assert_ne!(renewed.consented_at, first.consented_at);
        assert!(published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty());

        let cleared = save_for_client_dri(
            &fixture.surreal,
            fixture.client.id,
            fixture.project.id,
            &private_request("Edited private note."),
        )
        .await
        .unwrap();
        assert_eq!(cleared.quote, "Edited private note.");
        assert!(cleared.consented_at.is_none());
        assert!(cleared.published_at.is_none());
        assert!(published_for_home(&fixture.surreal, 10)
            .await
            .unwrap()
            .is_empty());
    }
}
