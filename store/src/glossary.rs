//! The firm glossary as **reference data** — authored in the repository,
//! materialized into rows (#894).
//!
//! Both, not either. `docs/glossary.md` stays the source of truth because
//! the glossary is where load-bearing distinctions live — that Lawyer
//! includes attorneys, that Participation is not the disclosures table —
//! and those get reviewed in pull requests. A controlled vocabulary that
//! changed through an admin form with no code review would be a real loss
//! for a legal product.
//!
//! Materializing it anyway buys the thing a file cannot: a dashboard
//! section, a notation template, or a questionnaire prompt can
//! **reference a term by slug** instead of restating it, so the
//! definition has exactly one home and every surface that shows it agrees.
//!
//! # This table lives in SurrealDB
//!
//! `glossary_terms` is a leaf reference table nothing links to and that
//! links to nothing. The unique `glossary_term_slug` index is what
//! enforces one row per slug; a violation has no typed detail, so
//! [`crate::surreal::retry::unique_violation`] discriminates on that index
//! name.
//!
//! # The distinction that must not be collapsed
//!
//! A contract's **defined terms** — the capitalized definitions scoped to
//! one document — are *matter data*: per-project, never seeded, covered by
//! the no-client-data rule. The **firm glossary** is reference data:
//! universal, identical in every deployment.
//!
//! They must never share a table. If they did, firm vocabulary would get
//! polluted with client content, and that is very hard to unwind. This
//! table makes the mistake unrepresentable rather than merely discouraged:
//! `glossary_term` has no `project_id`, no owner, and no link to any
//! matter-scoped table, so there is no field in which a client's defined
//! term could be written — `store/tests/glossary_terms.rs` pins that
//! shape against the applied schema. A component that renders both reads
//! two sources.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "glossary_term";

/// The authored glossary, embedded at compile time so a deployed binary
/// materializes the same vocabulary it was built from — no runtime lookup
/// of a file that might have drifted.
pub const GLOSSARY_MD: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/glossary.md"));

/// One parsed glossary term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// The published anchor (`lawyer-review`) — the stable reference key.
    pub slug: String,
    /// The heading text (`Lawyer Review`).
    pub title: String,
    /// The Markdown body beneath the heading.
    pub body: String,
}

/// One materialized glossary term row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GlossaryTerm {
    pub id: Uuid,
    /// The published anchor (`lawyer-review`) — the stable reference key.
    /// Unique.
    pub slug: String,
    /// The heading text as authored (`Lawyer Review`).
    pub title: String,
    /// The Markdown body beneath the heading.
    pub body: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it — the seam between
/// [`GlossaryTerm`] and the SDK's own `RecordId` and `Datetime`.
#[derive(SurrealValue)]
struct GlossaryTermRow {
    id: surrealdb::types::RecordId,
    slug: String,
    title: String,
    body: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl GlossaryTermRow {
    /// `None` when the record id is not a native UUID key — a row
    /// written by something that bypassed [`crate::surreal::record_id`].
    fn into_term(self) -> Option<GlossaryTerm> {
        Some(GlossaryTerm {
            id: record_uuid(&self.id)?,
            slug: self.slug,
            title: self.title,
            body: self.body,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`GlossaryTermRow`] from only one query.
const SELECT: &str = "id, slug, title, body, inserted_at, updated_at";

/// Errors reading or writing a glossary term.
#[derive(Debug, thiserror::Error)]
pub enum GlossaryError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `glossary_term_slug` — another row already
    /// holds this slug.
    #[error("that glossary slug is already in use")]
    SlugTaken,
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see [`GlossaryTermRow::into_term`].
    #[error("writing a glossary term returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no
/// typed detail** — the index name in the message is the only
/// discriminator through the shared classifier in [`crate::surreal::retry`].
fn classify_write(error: surrealdb::Error) -> GlossaryError {
    if crate::surreal::retry::unique_violation(&error) == Some("glossary_term_slug") {
        GlossaryError::SlugTaken
    } else {
        GlossaryError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, GlossaryError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Parse authored glossary Markdown into terms.
///
/// Each `## Heading` opens a term and everything up to the next `##` is
/// its body. The slug is the published-docs anchor shape, so a row's key
/// is the same string `/docs/glossary#<slug>` uses and a reference cannot
/// mean one thing in a row and another in a link.
#[must_use]
pub fn parse(markdown: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut title: Option<String> = None;
    let mut body = String::new();
    for line in markdown.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(previous) = title.replace(heading.trim().to_string()) {
                terms.push(term(previous, &body));
                body.clear();
            }
        } else if title.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(last) = title {
        terms.push(term(last, &body));
    }
    terms
}

fn term(title: String, body: &str) -> Term {
    Term {
        slug: slugify(&title),
        title,
        body: body.trim().to_string(),
    }
}

/// The published-docs anchor for a heading (`Lawyer Review` →
/// `lawyer-review`).
#[must_use]
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        } else if c == '-' || c == '_' {
            out.push(c);
        }
    }
    out
}

/// Materialize the authored glossary into `glossary_term` rows, keyed by
/// slug.
///
/// Idempotent by find-or-create plus update: re-running after an edit
/// updates the title and body in place rather than appending a second row
/// for the same term, so every boot converges on exactly the authored
/// vocabulary. Race-safe without a lock: a concurrent boot that wins the
/// `glossary_term_slug` unique index turns this call's insert into
/// [`GlossaryError::SlugTaken`], which is re-read as the winner's row and
/// updated in place. Returns the number of terms written.
///
/// # Errors
///
/// Propagates any database error.
pub async fn materialize(db: &SurrealDb, markdown: &str) -> Result<usize, GlossaryError> {
    let terms = parse(markdown);
    for t in &terms {
        match by_slug(db, &t.slug).await? {
            Some(existing) => {
                if existing.title != t.title || existing.body != t.body {
                    update(db, existing.id, t).await?;
                }
            }
            None => match create(db, t).await {
                Ok(()) => {}
                // A concurrent boot won the slug index between the check
                // and the insert; converge on the winner's row.
                Err(GlossaryError::SlugTaken) => {
                    let existing = by_slug(db, &t.slug)
                        .await?
                        .ok_or(GlossaryError::WriteReturnedNothing)?;
                    update(db, existing.id, t).await?;
                }
                Err(error) => return Err(error),
            },
        }
    }
    Ok(terms.len())
}

/// Insert one term row under a fresh v7 UUID record id.
async fn create(db: &SurrealDb, t: &Term) -> Result<(), GlossaryError> {
    let id = Uuid::now_v7();
    writing(|| {
        db.query("CREATE $id SET slug = $slug, title = $title, body = $body".to_string())
            .bind(("id", record_id(TABLE, id)))
            .bind(("slug", t.slug.clone()))
            .bind(("title", t.title.clone()))
            .bind(("body", t.body.clone()))
    })
    .await?;
    Ok(())
}

/// Bring an existing term row up to the authored text.
async fn update(db: &SurrealDb, id: Uuid, t: &Term) -> Result<(), GlossaryError> {
    writing(|| {
        db.query(
            "UPDATE $id SET title = $title, body = $body, updated_at = time::now()".to_string(),
        )
        .bind(("id", record_id(TABLE, id)))
        .bind(("title", t.title.clone()))
        .bind(("body", t.body.clone()))
    })
    .await?;
    Ok(())
}

/// Resolve one term by its slug — the lookup a composition performs when
/// it references a definition instead of restating it.
///
/// # Errors
///
/// Propagates any database error.
pub async fn by_slug(db: &SurrealDb, slug: &str) -> Result<Option<GlossaryTerm>, GlossaryError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE slug = $slug LIMIT 1"
        ))
        .bind(("slug", slug.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<GlossaryTermRow> = response.take(0)?;
    Ok(row.and_then(GlossaryTermRow::into_term))
}

/// Every materialized term, alphabetical by slug.
///
/// # Errors
///
/// Propagates any database error.
pub async fn all(db: &SurrealDb) -> Result<Vec<GlossaryTerm>, GlossaryError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY slug ASC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<GlossaryTermRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(GlossaryTermRow::into_term)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{parse, slugify, GLOSSARY_MD};

    #[test]
    fn slug_matches_the_published_docs_anchor_shape() {
        assert_eq!(slugify("Lawyer Review"), "lawyer-review");
        assert_eq!(slugify("Engagement / Retainer"), "engagement--retainer");
        assert_eq!(slugify("`ctx.run`"), "ctxrun");
    }

    #[test]
    fn the_authored_glossary_parses_into_terms() {
        let terms = parse(GLOSSARY_MD);
        assert!(
            terms.len() > 25,
            "expected the full authored vocabulary, got {}",
            terms.len()
        );
        assert!(terms.iter().any(|t| t.title == "Lawyer Review"));
        assert!(terms.iter().any(|t| t.title == "Template"));
        assert!(
            terms.iter().all(|t| !t.slug.is_empty()),
            "every term needs a reference key"
        );
    }

    #[test]
    fn slugs_are_unique_so_a_reference_is_unambiguous() {
        let terms = parse(GLOSSARY_MD);
        let mut slugs: Vec<&str> = terms.iter().map(|t| t.slug.as_str()).collect();
        slugs.sort_unstable();
        let before = slugs.len();
        slugs.dedup();
        assert_eq!(
            before,
            slugs.len(),
            "two terms sharing a slug would make a reference ambiguous"
        );
    }
}
