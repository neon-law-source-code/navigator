//! The firm glossary as **reference data** — authored in the repository,
//! materialized into rows (#894).
//!
//! Both, not either. `docs/glossary/` — one Markdown file per term —
//! stays the source of truth because
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

use std::fmt::Write as _;
use std::sync::LazyLock;

use include_dir::{include_dir, Dir};

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "glossary_term";

/// The authored glossary directory, embedded at compile time so a
/// deployed binary materializes the same vocabulary it was built from —
/// no runtime lookup of a file that might have drifted. One Markdown file
/// per term, `<slug>.md`, whose frontmatter carries the `title:`; the
/// directory's `README.md` is its preamble, not a term.
pub static GLOSSARY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../docs/glossary");

/// The authored glossary directory on disk — the same directory
/// [`GLOSSARY`] embeds, named by the same path so a writer and the gate
/// cannot point at different copies.
///
/// [`with_rendered_tables`] is checked against the embedded bytes by the
/// workspace gate, so a writer that resolved its own target from the
/// working directory could rewrite one tree while the gate kept reading
/// another. Sharing this constant is what makes that mismatch
/// unrepresentable.
pub const GLOSSARY_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/glossary");

/// How [`GLOSSARY_PATH`] is spelled in prose: the repository-relative path
/// a reader can act on, rather than the absolute build path.
pub const GLOSSARY_LABEL: &str = "docs/glossary";

/// The directory's preamble file — prose about the glossary, not a term.
pub const README: &str = "README.md";

/// The glossary preamble: the reader-facing opening of [`README`] — below
/// its `# Glossary` heading, which every surface replaces with its own
/// title, and above its first `## ` section, which is for authors.
#[must_use]
pub fn preamble() -> &'static str {
    let raw = GLOSSARY
        .get_file(README)
        .and_then(|file| file.contents_utf8())
        .unwrap_or("");
    let body = raw.trim_start().strip_prefix("# Glossary").unwrap_or(raw);
    body.split("\n## ").next().unwrap_or(body).trim()
}

/// Every authored term, alphabetical by slug.
///
/// # Panics
///
/// Panics when an embedded entry is malformed — no `title:`, or a file
/// name that is not its title's slug. The unit tests hold every
/// committed entry to that shape, so a panic here means a binary was
/// built from a tree the gate would have refused.
#[must_use]
pub fn terms() -> &'static [Term] {
    static TERMS: LazyLock<Vec<Term>> = LazyLock::new(|| {
        let mut terms: Vec<Term> = GLOSSARY
            .files()
            .filter_map(|file| {
                let name = file.path().file_name()?.to_str()?;
                let stem = name.strip_suffix(".md").filter(|_| name != README)?;
                let raw = file.contents_utf8()?;
                Some(
                    parse_entry(stem, raw)
                        .unwrap_or_else(|error| panic!("{GLOSSARY_LABEL}/{name}: {error}")),
                )
            })
            .collect();
        terms.sort_by(|a, b| a.slug.cmp(&b.slug));
        terms
    });
    &TERMS
}

/// The term a reader names — by title (any case) or by slug.
#[must_use]
pub fn find(needle: &str) -> Option<&'static Term> {
    let slug = slugify(needle);
    terms()
        .iter()
        .find(|term| term.title.eq_ignore_ascii_case(needle) || term.slug == slug)
}

/// Why one glossary file could not be read as a term.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EntryError {
    /// The file carries no leading frontmatter block.
    #[error("no frontmatter; an entry opens with `---`, a `title:` line, and `---`")]
    NoFrontmatter,
    /// The frontmatter has no usable `title:`.
    #[error("frontmatter has no `title:`")]
    NoTitle,
    /// The file name is not the slug of its title, so the anchor a link
    /// uses and the file a reader opens would disagree.
    #[error("file name `{stem}.md` must be `{slug}.md`, the slug of its title")]
    MisnamedFile { stem: String, slug: String },
}

#[derive(serde::Deserialize)]
struct EntryFrontmatter {
    title: Option<String>,
}

/// Parse one authored entry: `stem` is its file name without `.md`,
/// `raw` the whole file.
///
/// # Errors
///
/// [`EntryError`] when the frontmatter is missing, has no title, or the
/// file name is not the title's slug.
pub fn parse_entry(stem: &str, raw: &str) -> Result<Term, EntryError> {
    let (frontmatter, body) = rules::frontmatter::split(raw).ok_or(EntryError::NoFrontmatter)?;
    let title = serde_yaml::from_str::<EntryFrontmatter>(frontmatter)
        .ok()
        .and_then(|fm| fm.title)
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty())
        .ok_or(EntryError::NoTitle)?;
    let slug = slugify(&title);
    if slug != stem {
        return Err(EntryError::MisnamedFile {
            stem: stem.to_string(),
            slug,
        });
    }
    Ok(Term {
        slug,
        title,
        body: body.trim().to_string(),
    })
}

/// Where one link in an entry body points, resolved from the entry's own
/// directory (`docs/glossary/`) so every surface that renders the body
/// agrees on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget<'a> {
    /// A sibling entry, `matter.md` → the term `matter`; on the one-page
    /// glossary that is the in-page anchor `#matter`.
    Term(String),
    /// A file or directory elsewhere in the repository, as a
    /// repository-relative path (`store/src/persons.rs`, `store/`), plus
    /// the anchor it carried.
    Repository {
        path: String,
        anchor: Option<&'a str>,
    },
    /// An absolute URL, a site path, an in-page anchor, or a relative path
    /// that climbs out of the repository — passed through verbatim.
    Verbatim,
}

/// Resolve a link destination written in an entry body.
#[must_use]
pub fn link_target(dest: &str) -> LinkTarget<'_> {
    if dest.contains("://") || dest.starts_with("mailto:") || dest.starts_with('/') {
        return LinkTarget::Verbatim;
    }
    let (path, anchor) = dest
        .split_once('#')
        .map_or((dest, None), |(p, a)| (p, Some(a)));
    if path.is_empty() {
        return LinkTarget::Verbatim;
    }
    if let Some(stem) = path.strip_suffix(".md").filter(|s| !s.contains('/')) {
        if stem != "README" {
            return LinkTarget::Term(stem.to_string());
        }
    }
    let mut parts: Vec<&str> = GLOSSARY_LABEL.split('/').collect();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return LinkTarget::Verbatim;
                }
            }
            other => parts.push(other),
        }
    }
    let mut resolved = parts.join("/");
    if path.ends_with('/') {
        resolved.push('/');
    }
    LinkTarget::Repository {
        path: resolved,
        anchor,
    }
}

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

/// Glossary terms whose heading does not slug to the table they name.
///
/// The rule is the slug: `## Person` is the `person` table. These are
/// the terms the rule cannot reach — a heading that spells a join table
/// with an en dash, or one that reads as the domain noun rather than
/// the table name. A term absent from both the rule and this table
/// simply renders no box; only a wrong table here is drift, and
/// [`tests::every_alias_names_a_real_table`] pins that.
const TABLE_ALIASES: &[(&str, &str)] = &[
    ("Deadline", "statutory_deadline"),
    ("Docket Entry", "case_docket_entry"),
    ("External System Identity", "person_external_identity"),
    ("Person\u{2013}Entity Role", "entity_role"),
    ("Person\u{2013}Firm Role", "person_firm_role"),
    ("Person\u{2013}Project Role", "person_project_role"),
    ("Relationship Edge", "relationship"),
    ("Repository", "git_repository"),
];

/// The Surreal table a glossary term names, if it names one.
///
/// A term earns a schema box when its heading slugs to a table in the
/// shipped schema (`## Entity Type` → `entity_type`) or when
/// [`TABLE_ALIASES`] maps it. Everything else — a workflow prefix, a
/// role, a piece of vocabulary with no row behind it — returns `None`.
#[must_use]
pub fn table_for_term(title: &str) -> Option<String> {
    let candidate = TABLE_ALIASES
        .iter()
        .find(|(term, _)| *term == title)
        .map_or_else(
            || slugify(title).replace('-', "_"),
            |(_, t)| (*t).to_string(),
        );
    crate::schema::table_names()
        .into_iter()
        .find(|table| *table == candidate)
}

/// The opening character of a rendered schema box, and the marker
/// [`with_rendered_tables`] looks for inside a `text` fence.
const BOX_CORNER: char = '\u{250c}';

/// Render one table as a box of columns.
///
/// Columns come from [`crate::schema::table_columns`], so the box is
/// the shipped schema rather than a description of it: name column and
/// type column are each padded to their widest entry, which keeps the
/// art aligned without hand-counting.
#[must_use]
pub fn render_table_box(table: &str) -> Option<String> {
    let columns = crate::schema::table_columns(table);
    if columns.is_empty() {
        return None;
    }
    let name_width = columns.iter().map(|(n, _)| n.chars().count()).max()?;
    let type_width = columns.iter().map(|(_, t)| t.chars().count()).max()?;
    // The row between the two borders: " " + name + "  " + type + " ".
    let inner = 1 + name_width + 2 + type_width + 1;

    let head = format!("{BOX_CORNER}\u{2500} {table} ");
    let head_width = table.chars().count() + 4;
    let mut out = head;
    for _ in head_width..=inner {
        out.push('\u{2500}');
    }
    out.push('\u{2510}');
    out.push('\n');
    for (name, ty) in &columns {
        let _ = writeln!(
            out,
            "\u{2502} {name:name_width$}  {ty:type_width$} \u{2502}"
        );
    }
    out.push('\u{2514}');
    for _ in 0..inner {
        out.push('\u{2500}');
    }
    out.push('\u{2518}');
    out.push('\n');
    Some(out)
}

/// The fence a rendered schema box is written inside.
const BOX_FENCE: &str = "```text";

/// One entry's body with its schema box rewritten from the shipped
/// schema.
///
/// An entry keeps at most one box, at the end of its body: the prose
/// says what the noun means, the box says what the row holds. A term
/// that no longer names a table loses its box, and a term that gained
/// one grows it, so an entry cannot drift from `navigator.surql`
/// without this function's output changing.
#[must_use]
pub fn with_rendered_table(title: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept = strip_table_box(&lines);
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if let Some(rendered) = table_for_term(title).and_then(|t| render_table_box(&t)) {
        let _ = write!(out, "\n\n{BOX_FENCE}\n{rendered}```");
    }
    out
}

/// One authored entry file with its schema box rewritten — the
/// frontmatter is kept byte for byte, and the file ends in exactly one
/// newline (`M047`). `None` when the file is not a well-formed entry.
#[must_use]
pub fn rewrite_entry(stem: &str, raw: &str) -> Option<String> {
    let term = parse_entry(stem, raw).ok()?;
    let (_, body) = rules::frontmatter::split(raw)?;
    let head = &raw[..raw.len() - body.len()];
    Some(format!(
        "{head}\n\n{body}\n",
        head = head.trim_end(),
        body = with_rendered_table(&term.title, &term.body)
    ))
}

/// One body with its generated box (if any) removed.
fn strip_table_box<'a>(section: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::with_capacity(section.len());
    let mut index = 0;
    while index < section.len() {
        let opens_box = section[index] == BOX_FENCE
            && section
                .get(index + 1)
                .is_some_and(|l| l.starts_with(BOX_CORNER));
        if opens_box {
            while out.last().is_some_and(|l| l.is_empty()) {
                out.pop();
            }
            index += 1;
            while index < section.len() && section[index] != "```" {
                index += 1;
            }
            index += 1;
            continue;
        }
        out.push(section[index]);
        index += 1;
    }
    out
}

/// Materialize glossary terms — [`terms`] in the canonical seed — into
/// `glossary_term` rows, keyed by slug.
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
pub async fn materialize(db: &SurrealDb, terms: &[Term]) -> Result<usize, GlossaryError> {
    for t in terms {
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
    use super::{
        find, parse_entry, preamble, render_table_box, rewrite_entry, slugify, table_for_term,
        terms, EntryError, GLOSSARY, GLOSSARY_LABEL, GLOSSARY_PATH, README, TABLE_ALIASES,
    };

    /// An alias naming a table the schema does not define would render no
    /// box at all, silently — the term would just look like vocabulary.
    #[test]
    fn every_alias_names_a_real_table() {
        let tables = crate::schema::table_names();
        for (term, table) in TABLE_ALIASES {
            assert!(
                tables.iter().any(|t| t == table),
                "alias `{term}` names `{table}`, which is not a table in the shipped schema"
            );
        }
    }

    /// Two terms claiming one table would render the same box twice and
    /// leave a reader unsure which noun owns the row.
    #[test]
    fn no_two_terms_claim_the_same_table() {
        let mut claimed: Vec<(String, String)> = Vec::new();
        for term in terms() {
            if let Some(table) = table_for_term(&term.title) {
                if let Some((other, _)) = claimed.iter().find(|(_, t)| *t == table) {
                    panic!("`{}` and `{other}` both claim table `{table}`", term.title);
                }
                claimed.push((term.title.clone(), table));
            }
        }
        assert!(
            claimed.len() > 20,
            "the glossary should carry a schema box for most tables it names, got {}",
            claimed.len()
        );
    }

    /// The rule before the aliases: a heading that slugs to a table is
    /// that table, and one that does not is not.
    #[test]
    fn table_for_term_follows_the_slug() {
        assert_eq!(table_for_term("Person").as_deref(), Some("person"));
        assert_eq!(
            table_for_term("Entity Type").as_deref(),
            Some("entity_type")
        );
        assert_eq!(
            table_for_term("Person\u{2013}Project Role").as_deref(),
            Some("person_project_role")
        );
        assert_eq!(table_for_term("Council"), None);
        assert_eq!(table_for_term("Lawyer Review"), None);
    }

    /// Every row is padded to the same width, so the box closes.
    #[test]
    fn render_table_box_is_square() {
        let rendered = render_table_box("schema_version").expect("schema_version is a table");
        let widths: Vec<usize> = rendered.lines().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged box: {widths:?}\n{rendered}"
        );
        assert_eq!(render_table_box("not_a_table"), None);
    }

    /// Every embedded entry file, as `(stem, raw)`, `README.md` excluded.
    fn entry_files() -> Vec<(String, &'static str)> {
        GLOSSARY
            .files()
            .filter_map(|file| {
                let name = file.path().file_name()?.to_str()?;
                let stem = name.strip_suffix(".md").filter(|_| name != README)?;
                Some((stem.to_string(), file.contents_utf8()?))
            })
            .collect()
    }

    /// The strict drift gate: the boxes in the entries are the schema.
    ///
    /// A `DEFINE FIELD` added, retyped, or removed in `navigator.surql`
    /// fails this until `navigator glossary tables --write` reruns, which
    /// is the whole point of generating them.
    #[test]
    fn glossary_schema_boxes_are_current() {
        for (stem, raw) in entry_files() {
            assert_eq!(
                rewrite_entry(&stem, raw).as_deref(),
                Some(raw),
                "{GLOSSARY_LABEL}/{stem}.md schema box is stale; \
                 re-run `navigator glossary tables --write`"
            );
        }
    }

    /// Rewriting twice changes nothing the first pass did not.
    #[test]
    fn rendering_tables_is_idempotent() {
        for (stem, raw) in entry_files() {
            let once = rewrite_entry(&stem, raw).expect("well-formed entry");
            assert_eq!(rewrite_entry(&stem, &once).as_deref(), Some(once.as_str()));
        }
    }

    /// `glossary tables --write` writes under [`GLOSSARY_PATH`] while the
    /// gate compares what [`GLOSSARY`] embedded. If the two ever named
    /// different trees the writer would rewrite one copy and the gate would
    /// keep failing on the other, so hold them to the same bytes.
    #[test]
    fn the_glossary_path_names_the_directory_the_glossary_embeds() {
        for (stem, raw) in entry_files() {
            let on_disk = std::fs::read_to_string(format!("{GLOSSARY_PATH}/{stem}.md"))
                .expect("GLOSSARY_PATH must hold every embedded entry");
            assert_eq!(on_disk, raw, "GLOSSARY_PATH and GLOSSARY diverge on {stem}");
        }
        assert!(
            GLOSSARY_PATH.ends_with(GLOSSARY_LABEL),
            "the prose label must be how GLOSSARY_PATH actually ends, got {GLOSSARY_PATH}"
        );
    }

    #[test]
    fn slug_matches_the_published_anchor_shape() {
        assert_eq!(slugify("Lawyer Review"), "lawyer-review");
        assert_eq!(slugify("Engagement / Retainer"), "engagement--retainer");
        assert_eq!(slugify("`ctx.run`"), "ctxrun");
    }

    #[test]
    fn the_authored_glossary_reads_into_terms() {
        let terms = terms();
        assert!(
            terms.len() > 25,
            "expected the full authored vocabulary, got {}",
            terms.len()
        );
        assert!(terms.iter().any(|t| t.title == "Lawyer Review"));
        assert!(terms.iter().any(|t| t.title == "Template"));
        assert!(
            terms.iter().all(|t| !t.body.is_empty()),
            "every term needs a definition"
        );
        assert!(
            terms.windows(2).all(|w| w[0].slug < w[1].slug),
            "terms are alphabetical by slug, and a slug names one term"
        );
    }

    /// Every entry file parses: a bad file panics [`terms`] at boot, so
    /// this names the file instead.
    #[test]
    fn every_entry_file_is_a_well_formed_term() {
        for (stem, raw) in entry_files() {
            if let Err(error) = parse_entry(&stem, raw) {
                panic!("{GLOSSARY_LABEL}/{stem}.md: {error}");
            }
        }
    }

    #[test]
    fn an_entry_needs_a_title_that_slugs_to_its_file_name() {
        assert_eq!(
            parse_entry("x", "no frontmatter\n"),
            Err(EntryError::NoFrontmatter)
        );
        assert_eq!(
            parse_entry("x", "---\nother: 1\n---\n\nBody.\n"),
            Err(EntryError::NoTitle)
        );
        assert_eq!(
            parse_entry("lawyer", "---\ntitle: Lawyer Review\n---\n\nBody.\n"),
            Err(EntryError::MisnamedFile {
                stem: "lawyer".to_string(),
                slug: "lawyer-review".to_string()
            })
        );
        let term = parse_entry("lawyer-review", "---\ntitle: Lawyer Review\n---\n\nBody.\n")
            .expect("well-formed");
        assert_eq!(term.title, "Lawyer Review");
        assert_eq!(term.body, "Body.");
    }

    #[test]
    fn find_matches_a_title_or_a_slug() {
        assert_eq!(
            find("lawyer review").map(|t| t.slug.as_str()),
            Some("lawyer-review")
        );
        assert_eq!(find("ctxrun").map(|t| t.title.as_str()), Some("`ctx.run`"));
        assert!(find("not a real term").is_none());
    }

    #[test]
    fn links_resolve_from_the_glossary_directory() {
        use super::{link_target, LinkTarget};
        assert_eq!(link_target("matter.md"), LinkTarget::Term("matter".into()));
        assert_eq!(
            link_target("../../store/src/persons.rs"),
            LinkTarget::Repository {
                path: "store/src/persons.rs".into(),
                anchor: None
            }
        );
        assert_eq!(
            link_target("../../store/"),
            LinkTarget::Repository {
                path: "store/".into(),
                anchor: None
            }
        );
        assert_eq!(
            link_target("../notation.md#template"),
            LinkTarget::Repository {
                path: "docs/notation.md".into(),
                anchor: Some("template")
            }
        );
        assert_eq!(
            link_target("README.md"),
            LinkTarget::Repository {
                path: "docs/glossary/README.md".into(),
                anchor: None
            }
        );
        for verbatim in [
            "https://restate.dev",
            "mailto:a@b.c",
            "/glossary",
            "#x",
            "../../../x",
        ] {
            assert_eq!(link_target(verbatim), LinkTarget::Verbatim, "{verbatim}");
        }
    }

    #[test]
    fn every_sibling_link_names_a_real_term() {
        let slugs: Vec<&str> = terms().iter().map(|t| t.slug.as_str()).collect();
        for term in terms() {
            let mut rest = term.body.as_str();
            while let Some(at) = rest.find("](") {
                rest = &rest[at + 2..];
                let Some(close) = rest.find(')') else { break };
                if let super::LinkTarget::Term(slug) = super::link_target(&rest[..close]) {
                    assert!(
                        slugs.contains(&slug.as_str()),
                        "`{}` links `{slug}.md`, which is not a glossary term",
                        term.title
                    );
                }
            }
        }
    }

    #[test]
    fn the_preamble_is_the_readers_opening_only() {
        assert!(!preamble().is_empty());
        assert!(!preamble().starts_with('#'));
        assert!(
            !preamble().contains("## ") && !preamble().contains("To add a term"),
            "authoring notes stay in the README: {}",
            preamble()
        );
    }
}
