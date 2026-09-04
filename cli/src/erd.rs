// `navigator erd` — introspect the schema in the store and
// emit a Mermaid `erDiagram` block on stdout. The output renders
// directly in GitHub markdown and any Mermaid-aware viewer.
//
// With `--format svg`, emits a hand-written SVG instead. The SVG
// renderer is **deterministic by construction**: integer-only
// arithmetic, alphabetical iteration via [`BTreeMap`], no timestamps,
// no random IDs. Same schema in → byte-identical SVG out. That
// invariant is asserted by `cli/tests/erd_svg.rs`.
//
// The schema comes from `INFO FOR DB` / `INFO FOR TABLE`, and a
// `record<…>` field type IS the foreign key — so column metadata and
// relationship discovery arrive in one introspection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::Context;
use clap::ValueEnum;
use store::surreal::SurrealDb;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    /// Mermaid `erDiagram` block (GitHub renders natively).
    Mermaid,
    /// Hand-written SVG, deterministic across runs.
    Svg,
}

#[derive(Clone)]
struct Column {
    name: String,
    ty: String,
    primary_key: bool,
}

#[derive(Clone)]
struct ForeignKey {
    from_column: String,
    to_table: String,
}

/// Introspected schema: table name → (columns, foreign keys). Sorted
/// alphabetically by table name (the [`BTreeMap`] guarantee), which
/// is the load-bearing property for deterministic SVG output.
type Schema = BTreeMap<String, (Vec<Column>, Vec<ForeignKey>)>;

/// Introspect the store and render the diagram.
pub async fn run_surreal(db: &SurrealDb, format: OutputFormat) -> anyhow::Result<()> {
    emit(&fetch_surreal_schema(db).await?, format);
    Ok(())
}

fn emit(schema: &Schema, format: OutputFormat) {
    let out = match format {
        OutputFormat::Mermaid => render_mermaid(schema),
        OutputFormat::Svg => render_svg(schema),
    };
    print!("{out}");
}

/// Introspect a `SurrealDB` database.
///
/// The engine's own description of the schema comes from
/// [`store::schema::introspect`]; everything below is parsing, which
/// is why it is pure and unit-tested against captured engine output.
///
/// This works because every table is `SCHEMAFULL`. A `SCHEMALESS`
/// table would report no fields and render as an empty box — which is
/// why the schema file defines them all `SCHEMAFULL`.
async fn fetch_surreal_schema(db: &SurrealDb) -> anyhow::Result<Schema> {
    let introspection = store::schema::introspect(db)
        .await
        .context("introspect the SurrealDB schema")?;
    Ok(introspection
        .iter()
        .map(|(table, definition)| (table.clone(), surreal_table(&definition.fields)))
        .collect())
}

/// Turn one table's `DEFINE FIELD` statements into columns and foreign
/// keys. Pure, so the parsing is unit-tested against captured engine
/// output rather than only against a live database.
fn surreal_table(fields: &BTreeMap<String, String>) -> (Vec<Column>, Vec<ForeignKey>) {
    // Every Surreal record has an `id`; it is implicit and so never
    // appears in the field list, but it is the primary key and the
    // diagram would be wrong without it.
    let mut columns = vec![Column {
        name: "id".to_string(),
        ty: "record".to_string(),
        primary_key: true,
    }];
    let mut foreign_keys = Vec::new();

    for (name, definition) in fields {
        let ty = surreal_field_type(definition);
        for target in record_targets(&ty) {
            foreign_keys.push(ForeignKey {
                from_column: name.clone(),
                to_table: target,
            });
        }
        columns.push(Column {
            name: name.clone(),
            ty: normalize_surreal_type(&ty),
            primary_key: false,
        });
    }

    (columns, foreign_keys)
}

/// The type in a `DEFINE FIELD` statement: everything between `TYPE`
/// and whichever clause comes next.
fn surreal_field_type(definition: &str) -> String {
    const NEXT_CLAUSE: [&str; 8] = [
        " PERMISSIONS ",
        " ASSERT ",
        " DEFAULT ",
        " VALUE ",
        " READONLY",
        " REFERENCE ",
        " COMMENT ",
        " FLEXIBLE",
    ];
    let Some(rest) = definition.split_once(" TYPE ").map(|(_, rest)| rest) else {
        return String::new();
    };
    let end = NEXT_CLAUSE
        .iter()
        .filter_map(|clause| rest.find(clause))
        .min()
        .unwrap_or(rest.len());
    rest[..end].trim().to_string()
}

/// The tables a field points at. `record<entity>` names one;
/// `record<person | entity>` — the polymorphic edge ends — names two.
fn record_targets(ty: &str) -> Vec<String> {
    let Some(start) = ty.find("record<") else {
        return Vec::new();
    };
    let inner = &ty[start + "record<".len()..];
    let Some(end) = inner.find('>') else {
        return Vec::new();
    };
    inner[..end]
        .split('|')
        .map(|target| target.trim().to_string())
        .filter(|target| !target.is_empty())
        .collect()
}

/// Mermaid attribute types are bare words, so a Surreal type has to
/// lose its punctuation to survive rendering: `record<person | entity>`
/// would otherwise break the diagram it appears in. An optional field
/// reads back from the engine as `none | string`, which becomes
/// `option_string` rather than a leading underscore.
fn normalize_surreal_type(ty: &str) -> String {
    let ty = match ty.trim().strip_prefix("none | ") {
        Some(inner) => format!("option {inner}"),
        None => ty.trim().to_string(),
    };
    let mut out = String::with_capacity(ty.len());
    for ch in ty.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn render_mermaid(schema: &Schema) -> String {
    let mut out = String::from("erDiagram\n");

    let fk_columns: BTreeMap<&str, Vec<&str>> = schema
        .iter()
        .map(|(table, (_, fks))| {
            (
                table.as_str(),
                fks.iter().map(|f| f.from_column.as_str()).collect(),
            )
        })
        .collect();

    for (table, (cols, _)) in schema {
        let _ = writeln!(out, "    {table} {{");
        for col in cols {
            let role = if col.primary_key {
                " PK"
            } else if fk_columns
                .get(table.as_str())
                .is_some_and(|fks| fks.contains(&col.name.as_str()))
            {
                " FK"
            } else {
                ""
            };
            let ty = if col.ty.is_empty() {
                "TEXT"
            } else {
                col.ty.as_str()
            };
            let _ = writeln!(out, "        {ty} {name}{role}", name = col.name);
        }
        let _ = writeln!(out, "    }}");
    }

    for (table, (_, fks)) in schema {
        for fk in fks {
            let _ = writeln!(
                out,
                "    {parent} ||--o{{ {child} : \"{col}\"",
                parent = fk.to_table,
                child = table,
                col = fk.from_column,
            );
        }
    }
    out
}

// === SVG renderer (deterministic) =========================================
//
// Layout: alphabetical row-major grid, 4 columns wide. Per-column widths
// scale with content; per-row heights scale with the tallest table in
// that row. Edges are straight lines from the FK column's right midpoint
// to the parent table's left edge at the title bar's midpoint. All
// dimensions are integers; all iteration goes through [`BTreeMap`] /
// [`BTreeSet`] so order is deterministic. No timestamps, no random IDs.

const CHAR_WIDTH: i32 = 8;
const ROW_HEIGHT: i32 = 22;
const TITLE_HEIGHT: i32 = 30;
const CELL_PAD: i32 = 12;
const CELL_GAP_X: i32 = 40;
const CELL_GAP_Y: i32 = 24;
const GRID_COLS: usize = 4;
const MARGIN: i32 = 30;
const FONT_SIZE: i32 = 13;

struct PlacedTable<'a> {
    name: &'a str,
    cols: &'a [Column],
    fk_columns: BTreeSet<&'a str>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct Layout<'a> {
    tables: Vec<PlacedTable<'a>>,
    canvas_w: i32,
    canvas_h: i32,
}

/// Cast a `usize` to `i32` for layout math. Every input is a small
/// count (table count, column index, grid dimension) bounded well
/// under `i32::MAX` in practice; the cast is deliberate because we
/// want integer arithmetic for byte-deterministic SVG output.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
const fn i32_of(n: usize) -> i32 {
    n as i32
}

/// Cell text for a column's type: lower-case, so the diagram reads the
/// same however the engine spelled it back.
fn display_type(t: &str) -> String {
    t.to_lowercase()
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn natural_table_size(name: &str, cols: &[Column], fk_set: &BTreeSet<&str>) -> (i32, i32) {
    let title_chars = i32_of(name.chars().count());
    let mut max_row_chars = title_chars;
    for c in cols {
        let role_len = if c.primary_key || fk_set.contains(c.name.as_str()) {
            3 // " PK" or " FK"
        } else {
            0
        };
        let ty_norm = display_type(&c.ty);
        // "<name>  <type><role>" — two spaces between name and type for
        // breathing room in the rendered text.
        let chars = i32_of(c.name.chars().count()) + 2 + i32_of(ty_norm.chars().count()) + role_len;
        if chars > max_row_chars {
            max_row_chars = chars;
        }
    }
    let w = max_row_chars * CHAR_WIDTH + 2 * CELL_PAD;
    let h = TITLE_HEIGHT + ROW_HEIGHT * i32_of(cols.len());
    (w, h)
}

struct Sized<'a> {
    name: &'a str,
    cols: &'a [Column],
    fk_set: BTreeSet<&'a str>,
    w: i32,
    h: i32,
}

fn compute_layout(schema: &Schema) -> Layout<'_> {
    // First pass: compute every table's natural size and its FK column
    // set (needed both for size calculation and for later rendering).
    let mut sized: Vec<Sized<'_>> = Vec::new();
    for (name, (cols, fks)) in schema {
        let fk_set: BTreeSet<&str> = fks.iter().map(|f| f.from_column.as_str()).collect();
        let (w, h) = natural_table_size(name, cols, &fk_set);
        sized.push(Sized {
            name: name.as_str(),
            cols: cols.as_slice(),
            fk_set,
            w,
            h,
        });
    }

    // Second pass: per-grid-column widths and per-grid-row heights.
    let row_count = sized.len().div_ceil(GRID_COLS);
    let mut col_widths = [0i32; GRID_COLS];
    let mut row_heights = vec![0i32; row_count];
    for (i, s) in sized.iter().enumerate() {
        let col = i % GRID_COLS;
        let row = i / GRID_COLS;
        if s.w > col_widths[col] {
            col_widths[col] = s.w;
        }
        if s.h > row_heights[row] {
            row_heights[row] = s.h;
        }
    }

    // Third pass: place each table at its grid cell's top-left.
    let mut tables: Vec<PlacedTable<'_>> = Vec::with_capacity(sized.len());
    for (i, s) in sized.into_iter().enumerate() {
        let col = i % GRID_COLS;
        let row = i / GRID_COLS;
        let x = MARGIN + (0..col).map(|c| col_widths[c] + CELL_GAP_X).sum::<i32>();
        let y = MARGIN + (0..row).map(|r| row_heights[r] + CELL_GAP_Y).sum::<i32>();
        tables.push(PlacedTable {
            name: s.name,
            cols: s.cols,
            fk_columns: s.fk_set,
            x,
            y,
            w: s.w,
            h: s.h,
        });
    }

    let canvas_w =
        MARGIN * 2 + col_widths.iter().sum::<i32>() + CELL_GAP_X * (i32_of(GRID_COLS) - 1);
    let canvas_h = MARGIN * 2
        + row_heights.iter().sum::<i32>()
        + CELL_GAP_Y * (i32_of(row_heights.len()) - 1).max(0);

    Layout {
        tables,
        canvas_w,
        canvas_h,
    }
}

fn emit_edges(out: &mut String, schema: &Schema, by_name: &BTreeMap<&str, &PlacedTable<'_>>) {
    out.push_str(r#"<g class="edges">"#);
    out.push('\n');
    for (name, (_, fks)) in schema {
        let Some(src) = by_name.get(name.as_str()) else {
            continue;
        };
        for fk in fks {
            let Some(row_idx) = src.cols.iter().position(|c| c.name == fk.from_column) else {
                continue;
            };
            let Some(parent) = by_name.get(fk.to_table.as_str()) else {
                continue;
            };
            let x1 = src.x + src.w;
            let y1 = src.y + TITLE_HEIGHT + ROW_HEIGHT * i32_of(row_idx) + ROW_HEIGHT / 2;
            let x2 = parent.x;
            let y2 = parent.y + TITLE_HEIGHT / 2;
            let _ = writeln!(
                out,
                r#"<path class="e" d="M{x1},{y1} L{x2},{y2}" marker-end="url(#arrow)"/>"#
            );
        }
    }
    out.push_str("</g>\n");
}

fn emit_table(out: &mut String, p: &PlacedTable<'_>) {
    let _ = writeln!(out, r#"<g transform="translate({},{})">"#, p.x, p.y);
    let _ = writeln!(out, r#"<rect class="t" width="{}" height="{}"/>"#, p.w, p.h);
    let _ = writeln!(
        out,
        r#"<rect class="tt" width="{}" height="{}"/>"#,
        p.w, TITLE_HEIGHT
    );
    let _ = writeln!(
        out,
        r#"<text class="tn" x="{}" y="{}" text-anchor="middle">{}</text>"#,
        p.w / 2,
        TITLE_HEIGHT - 10,
        xml_escape(p.name)
    );
    for (i, col) in p.cols.iter().enumerate() {
        let y = TITLE_HEIGHT + ROW_HEIGHT * i32_of(i) + ROW_HEIGHT - 7;
        let ty_norm = display_type(&col.ty);
        let (suffix, class) = if col.primary_key {
            (" PK", "cpk")
        } else if p.fk_columns.contains(col.name.as_str()) {
            (" FK", "cfk")
        } else {
            ("", "ct")
        };
        let _ = writeln!(
            out,
            r#"<text class="cn" x="{}" y="{}">{}</text>"#,
            CELL_PAD,
            y,
            xml_escape(&col.name)
        );
        let _ = writeln!(
            out,
            r#"<text class="{}" x="{}" y="{}" text-anchor="end">{}{}</text>"#,
            class,
            p.w - CELL_PAD,
            y,
            xml_escape(&ty_norm),
            xml_escape(suffix),
        );
    }
    out.push_str("</g>\n");
}

fn render_svg(schema: &Schema) -> String {
    let layout = compute_layout(schema);
    let Layout {
        tables,
        canvas_w,
        canvas_h,
    } = &layout;

    let by_name: BTreeMap<&str, &PlacedTable<'_>> = tables.iter().map(|p| (p.name, p)).collect();

    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{canvas_w}" height="{canvas_h}" viewBox="0 0 {canvas_w} {canvas_h}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="{FONT_SIZE}">"#,
    );
    out.push_str(
        r##"<defs><marker id="arrow" markerWidth="10" markerHeight="10" refX="9" refY="3" orient="auto" markerUnits="strokeWidth"><path d="M0,0 L0,6 L9,3 z" fill="#666"/></marker></defs>
"##,
    );
    out.push_str(
        "<style>.t{fill:#fff;stroke:#666;stroke-width:1}.tt{fill:#eef;stroke:#666;stroke-width:1}.tn{fill:#222;font-weight:600}.cn{fill:#222}.ct{fill:#888}.cpk{fill:#c33;font-weight:600}.cfk{fill:#36c}.e{fill:none;stroke:#888;stroke-width:1.2}</style>\n",
    );

    emit_edges(&mut out, schema, &by_name);

    out.push_str(r#"<g class="tables">"#);
    out.push('\n');
    for p in tables {
        emit_table(&mut out, p);
    }
    out.push_str("</g>\n");

    out.push_str("</svg>\n");
    out
}

// Tests for the SVG renderer use synthetic schemas — pure-function
// checks that don't require a database.
#[cfg(test)]
mod tests {
    use super::*;

    /// Definitions captured verbatim from `INFO FOR TABLE` on a live
    /// engine running `store/src/schema/navigator.surql`. Parsing is
    /// pinned against what the engine actually emits — which is not
    /// what was written: `option<string>` reads back as `none | string`,
    /// and every field carries a trailing `PERMISSIONS` clause.
    fn captured_relationship_fields() -> BTreeMap<String, String> {
        [
            ("confidence_pct", "DEFINE FIELD confidence_pct ON relationship TYPE int ASSERT $value >= 0 AND $value <= 100 PERMISSIONS FULL"),
            ("detail", "DEFINE FIELD detail ON relationship TYPE none | string PERMISSIONS FULL"),
            ("in", "DEFINE FIELD in ON relationship TYPE record<person | entity> PERMISSIONS FULL"),
            ("inserted_at", "DEFINE FIELD inserted_at ON relationship TYPE datetime DEFAULT time::now() PERMISSIONS FULL"),
            ("kind", "DEFINE FIELD kind ON relationship TYPE string PERMISSIONS FULL"),
            ("out", "DEFINE FIELD out ON relationship TYPE record<person | entity> PERMISSIONS FULL"),
        ]
        .into_iter()
        .map(|(name, definition)| (name.to_string(), definition.to_string()))
        .collect()
    }

    #[test]
    fn a_surreal_field_type_stops_at_the_next_clause() {
        let cases = [
            ("DEFINE FIELD kind ON relationship TYPE string PERMISSIONS FULL", "string"),
            (
                "DEFINE FIELD confidence_pct ON relationship TYPE int ASSERT $value >= 0 PERMISSIONS FULL",
                "int",
            ),
            (
                "DEFINE FIELD inserted_at ON person TYPE datetime DEFAULT time::now() PERMISSIONS FULL",
                "datetime",
            ),
            (
                "DEFINE FIELD in ON relationship TYPE record<person | entity> PERMISSIONS FULL",
                "record<person | entity>",
            ),
            ("DEFINE FIELD detail ON relationship TYPE none | string", "none | string"),
            // A definition with no TYPE at all yields no type rather
            // than a panic — a FLEXIBLE or computed field.
            ("DEFINE FIELD anything ON t FLEXIBLE PERMISSIONS FULL", ""),
        ];
        for (definition, expected) in cases {
            assert_eq!(surreal_field_type(definition), expected, "{definition}");
        }
    }

    /// A `record<…>` type IS the foreign key, so this is the whole of
    /// relationship discovery.
    #[test]
    fn record_types_name_every_table_they_can_point_at() {
        assert_eq!(record_targets("record<entity>"), vec!["entity"]);
        assert_eq!(
            record_targets("record<person | entity>"),
            vec!["person", "entity"]
        );
        assert_eq!(record_targets("none | record<entity>"), vec!["entity"]);
        assert!(record_targets("string").is_empty());
        assert!(record_targets("datetime").is_empty());
        // Malformed input is not a diagram-breaking panic.
        assert!(record_targets("record<unterminated").is_empty());
    }

    /// Mermaid attribute types are bare words: an unsanitized
    /// `record<person | entity>` breaks the whole diagram it lands in.
    #[test]
    fn surreal_types_are_sanitized_into_mermaid_safe_words() {
        let cases = [
            ("string", "string"),
            ("int", "int"),
            ("datetime", "datetime"),
            ("record<entity>", "record_entity"),
            ("record<person | entity>", "record_person_entity"),
            ("none | string", "option_string"),
            ("", ""),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize_surreal_type(raw), expected, "{raw}");
        }
        assert!(
            normalize_surreal_type("record<person | entity>")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "a rendered type must be a bare word"
        );
    }

    #[test]
    fn a_relation_table_becomes_columns_plus_one_key_per_edge_end() {
        let (columns, foreign_keys) = surreal_table(&captured_relationship_fields());

        // `id` is implicit in Surreal — never in the field list, always
        // the primary key.
        assert_eq!(columns[0].name, "id");
        assert!(columns[0].primary_key);
        assert!(columns.iter().skip(1).all(|c| !c.primary_key));
        assert_eq!(
            columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec![
                "id",
                "confidence_pct",
                "detail",
                "in",
                "inserted_at",
                "kind",
                "out"
            ]
        );

        // Both ends are polymorphic, so each yields one key per target:
        // the diagram draws relationship→person and relationship→entity
        // from each of `in` and `out`.
        assert_eq!(
            foreign_keys
                .iter()
                .map(|fk| (fk.from_column.as_str(), fk.to_table.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("in", "person"),
                ("in", "entity"),
                ("out", "person"),
                ("out", "entity"),
            ]
        );
    }

    /// The renderers take a `Schema`, not an engine, so an introspected
    /// schema renders through the same Mermaid path a hand-built one does.
    #[test]
    fn a_surreal_schema_renders_through_the_shared_mermaid_renderer() {
        let mut schema = Schema::new();
        schema.insert(
            "relationship".to_string(),
            surreal_table(&captured_relationship_fields()),
        );

        let out = render_mermaid(&schema);

        assert!(out.starts_with("erDiagram\n"), "{out}");
        assert!(out.contains("        record id PK\n"), "{out}");
        assert!(
            out.contains("        record_person_entity in FK\n"),
            "{out}"
        );
        assert!(out.contains("        option_string detail\n"), "{out}");
        assert!(
            out.contains("    person ||--o{ relationship : \"in\""),
            "{out}"
        );
        assert!(
            out.contains("    entity ||--o{ relationship : \"out\""),
            "{out}"
        );
    }

    fn col(name: &str, ty: &str, pk: bool) -> Column {
        Column {
            name: name.into(),
            ty: ty.into(),
            primary_key: pk,
        }
    }

    fn fk(from: &str, to: &str) -> ForeignKey {
        ForeignKey {
            from_column: from.into(),
            to_table: to.into(),
        }
    }

    #[test]
    fn render_svg_is_byte_identical_across_runs() {
        let mut schema = Schema::new();
        schema.insert(
            "parents".into(),
            (
                vec![col("id", "uuid", true), col("name", "varchar", false)],
                vec![],
            ),
        );
        schema.insert(
            "children".into(),
            (
                vec![
                    col("id", "uuid", true),
                    col("parent_id", "uuid", false),
                    col("note", "text", false),
                ],
                vec![fk("parent_id", "parents")],
            ),
        );
        let first = render_svg(&schema);
        let second = render_svg(&schema);
        let third = render_svg(&schema);
        assert_eq!(first, second);
        assert_eq!(second, third);
    }

    #[test]
    fn render_svg_emits_expected_structure() {
        let mut schema = Schema::new();
        schema.insert(
            "people".into(),
            (
                vec![col("id", "uuid", true), col("name", "varchar", false)],
                vec![],
            ),
        );
        let svg = render_svg(&schema);
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>\n"));
        assert!(
            svg.contains(">people<"),
            "table name should appear in output"
        );
        assert!(svg.contains(">id<"), "id column should appear");
        assert!(svg.contains(" PK<"), "PK marker should appear");
    }

    #[test]
    fn xml_escape_handles_all_five_entities() {
        assert_eq!(xml_escape("<a>"), "&lt;a&gt;");
        assert_eq!(xml_escape("\"&'"), "&quot;&amp;&apos;");
    }

    #[test]
    fn display_type_lower_cases_however_the_engine_spelled_it() {
        assert_eq!(display_type("UUID"), "uuid");
        assert_eq!(display_type("option_Datetime"), "option_datetime");
    }

    #[tokio::test]
    async fn mermaid_diagram_includes_firm_and_membership() {
        let db = store::test_support::mem_surreal().await;
        let schema = fetch_surreal_schema(&db)
            .await
            .expect("introspect the applied schema");
        let mermaid = render_mermaid(&schema);
        assert!(
            mermaid.contains("    firm {"),
            "firm table missing from ERD"
        );
        assert!(
            mermaid.contains("    person_firm_role {"),
            "person_firm_role table missing from ERD"
        );
        assert!(
            mermaid.contains("    firm_brand {"),
            "firm_brand table missing from ERD"
        );
        assert!(
            mermaid.contains("option_record_entity entity_id FK") && mermaid.contains("    firm {"),
            "firm.entity_id missing from ERD"
        );
        assert!(
            mermaid.contains("option_record_firm firm_id FK"),
            "project.firm_id missing from ERD"
        );
    }
}
