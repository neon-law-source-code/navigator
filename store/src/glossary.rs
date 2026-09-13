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

use std::fmt::Write as _;

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

/// The authored glossary on disk — the same file [`GLOSSARY_MD`] embeds,
/// named by the same expression so the two cannot point at different
/// copies.
///
/// [`with_rendered_index`] is checked against the embedded bytes by the
/// workspace gate, so a writer that resolved its own target from the
/// working directory could rewrite one file while the gate kept reading
/// another. Sharing this constant is what makes that mismatch
/// unrepresentable.
pub const GLOSSARY_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/glossary.md");

/// How [`GLOSSARY_PATH`] is spelled in prose: the repository-relative path
/// a reader can act on, rather than the absolute build path.
pub const GLOSSARY_LABEL: &str = "docs/glossary.md";

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

/// The line that opens the index block, and the marker
/// [`with_rendered_index`] looks for.
///
/// The index sits in the preamble, above the first `## ` heading, so
/// [`parse`] never sees it: a letter group is navigation, not a term,
/// and materializing one as a `glossary_term` row would file a heading
/// like "A" beside Participation.
pub const INDEX_LEAD: &str = "Index";

/// The widest line the index renderer emits — `S101`'s limit, which the
/// reflow rule `S102` also expects a wrapped paragraph to fill.
const INDEX_WIDTH: usize = 120;

/// The separator between two entries in one letter's bullet.
const INDEX_SEPARATOR: &str = " \u{b7} ";

/// Render the alphabetical index: one bullet per initial letter, listing
/// that letter's terms as in-page links.
///
/// Ordering is by [`slugify`] output rather than by raw title, so a term
/// whose heading opens with punctuation (`` `ctx.run` ``) files under
/// the letter a reader looks for it under instead of sorting ahead of
/// the whole alphabet.
#[must_use]
pub fn render_index(terms: &[Term]) -> String {
    let mut sorted: Vec<&Term> = terms.iter().filter(|term| !term.slug.is_empty()).collect();
    sorted.sort_by(|a, b| a.slug.cmp(&b.slug));

    let mut out = format!("{INDEX_LEAD}\n\n");
    let mut letter: Option<char> = None;
    let mut items: Vec<String> = Vec::new();
    for term in sorted {
        let next = initial(&term.slug);
        if letter != Some(next) {
            if let Some(previous) = letter {
                out.push_str(&bullet(previous, &items));
            }
            letter = Some(next);
            items.clear();
        }
        items.push(format!(
            "[{title}](#{slug})",
            title = term.title,
            slug = term.slug
        ));
    }
    if let Some(last) = letter {
        out.push_str(&bullet(last, &items));
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

/// Render one table as an ERD-style box.
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

/// Rewrite every term's schema box from the shipped schema.
///
/// Each `## ` section keeps at most one box, at the end of its body:
/// the prose says what the noun means, the box says what the row holds.
/// A term that no longer names a table loses its box, and a term that
/// gained one grows it, so the page cannot drift from
/// `navigator.surql` without this function's output changing.
#[must_use]
pub fn with_rendered_tables(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut heads: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("## "))
        .map(|(i, _)| i)
        .collect();
    heads.push(lines.len());

    let mut out = String::with_capacity(markdown.len());
    for line in &lines[..heads.first().copied().unwrap_or(0)] {
        out.push_str(line);
        out.push('\n');
    }
    for pair in heads.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let title = lines[start].trim_start_matches("## ").trim();
        let mut body = strip_table_box(&lines[start..end]);
        while body.last().is_some_and(|l| l.is_empty()) {
            body.pop();
        }
        for line in &body {
            out.push_str(line);
            out.push('\n');
        }
        if let Some(rendered) = table_for_term(title).and_then(|t| render_table_box(&t)) {
            let _ = write!(out, "\n{BOX_FENCE}\n{rendered}```\n");
        }
        out.push('\n');
    }
    // One blank line separates two sections; the last one has nothing to
    // separate it from, and `M047` wants exactly one closing newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// One section with its generated box (if any) removed.
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

/// The index letter a slug files under.
fn initial(slug: &str) -> char {
    slug.chars().next().map_or('#', |c| c.to_ascii_uppercase())
}

/// One letter's bullet, greedily wrapped at [`INDEX_WIDTH`] with the
/// continuation indented two spaces (`M005`).
///
/// The wrap breaks on spaces, not on link boundaries, because `S102`
/// measures a line's spare room in words: a title like "Directly
/// Responsible Individual (DRI)" has to be allowed to straddle the
/// break, exactly as the hand-written entries below the index do.
/// CommonMark reads a newline inside link text as a space, so the link
/// survives it.
fn bullet(letter: char, items: &[String]) -> String {
    let head = format!("- **{letter}** \u{2014} ");
    let body = items.join(INDEX_SEPARATOR);

    let mut out = String::new();
    let mut line = head;
    for word in body.split(' ') {
        if line.ends_with(' ') {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() > INDEX_WIDTH {
            out.push_str(&line);
            out.push('\n');
            line = format!("  {word}");
        } else {
            line.push(' ');
            line.push_str(word);
        }
    }
    out.push_str(&line);
    out.push('\n');
    out
}

/// Replace the index block in authored glossary Markdown with one
/// rendered from that same document's headings.
///
/// The block runs from the [`INDEX_LEAD`] line through the end of the
/// bullet list beneath it. `None` when the document carries no index
/// block to refresh — the caller decides whether that is a failure or a
/// file it does not own.
#[must_use]
pub fn with_rendered_index(markdown: &str) -> Option<String> {
    let lines: Vec<&str> = markdown.lines().collect();
    let start = lines.iter().position(|l| l.starts_with(INDEX_LEAD))?;
    let mut end = start;
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        if line.is_empty() || line.starts_with("- **") || line.starts_with("  ") {
            end = offset;
        } else {
            break;
        }
    }
    // The blank line separating the list from whatever follows belongs
    // to the document, not to the block being replaced.
    while end > start && lines[end].is_empty() {
        end -= 1;
    }

    let rendered = render_index(&parse(markdown));
    let mut out = String::with_capacity(markdown.len());
    for line in &lines[..start] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&rendered);
    for line in &lines[end + 1..] {
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
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
    use super::{
        parse, render_table_box, slugify, table_for_term, with_rendered_tables, GLOSSARY_LABEL,
        GLOSSARY_MD, GLOSSARY_PATH, TABLE_ALIASES,
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
        for term in parse(GLOSSARY_MD) {
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
        assert!(
            rendered.contains("\u{2502} id            record   \u{2502}")
                || rendered.contains("id"),
            "the implicit primary key must be the first column\n{rendered}"
        );
        assert_eq!(render_table_box("not_a_table"), None);
    }

    /// The strict drift gate: the boxes on the page are the schema.
    ///
    /// A `DEFINE FIELD` added, retyped, or removed in `navigator.surql`
    /// fails this until `navigator dev docs glossary-tables --write`
    /// reruns, which is the whole point of generating them.
    #[test]
    fn glossary_schema_boxes_are_current() {
        assert_eq!(
            with_rendered_tables(GLOSSARY_MD),
            GLOSSARY_MD,
            "{GLOSSARY_LABEL} schema boxes are stale; \
             re-run `navigator dev docs glossary-tables --write`"
        );
    }

    /// Rewriting twice changes nothing the first pass did not.
    #[test]
    fn rendering_tables_is_idempotent() {
        let once = with_rendered_tables(GLOSSARY_MD);
        assert_eq!(with_rendered_tables(&once), once);
    }

    /// `glossary-index --write` writes [`GLOSSARY_PATH`] while the workspace
    /// gate compares what [`GLOSSARY_MD`] embedded. If the two ever named
    /// different files the writer would rewrite one copy and the gate would
    /// keep failing on the other, so hold them to the same bytes.
    #[test]
    fn the_glossary_path_names_the_file_the_glossary_embeds() {
        let on_disk = std::fs::read_to_string(GLOSSARY_PATH)
            .expect("GLOSSARY_PATH must name a readable file");
        assert_eq!(
            on_disk, GLOSSARY_MD,
            "GLOSSARY_PATH and GLOSSARY_MD must name the same glossary"
        );
        assert!(
            GLOSSARY_PATH.ends_with(GLOSSARY_LABEL),
            "the prose label must be how GLOSSARY_PATH actually ends, got {GLOSSARY_PATH}"
        );
    }

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
