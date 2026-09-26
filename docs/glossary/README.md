# Glossary

The vocabulary of Neon Law Navigator: every noun the product is built on, defined in one place. Most of these nouns are
also table names in [`store`](../../store/), and the definitions cite the canonical store module or Surreal schema, so a
reader can jump straight from term to schema. When an entry names a SurrealDB table or field, it uses the schema's
singular table name (for example, `person.role` and `person_project_role.participation`); plural `store::persons` and
`store::projects` are Rust module names.

## Authoring

One file per term. Each `<slug>.md` beside this README is one entry: its `title:` frontmatter is the term as a reader
says it, the file name is the stable reference key, and the body is the definition. The web publishes every entry on one
page at `/glossary`, anchored at `#<slug>`, and the CLI reads the same files through `navigator glossary list` and
`navigator glossary show <term>`.

To add a term, create `<slug>.md` whose name is the slug of its `title:` (`Lawyer Review` → `lawyer-review.md`). Its
frontmatter requires a `description:` with one plain-text sentence ending in a period and no Markdown links. The CLI
lists the title and description; `show` accepts the quoted title, regardless of case. Link other entries as siblings
(`[Matter](matter.md)`). When a table matters to a definition, link to its `DEFINE TABLE` statement in
`store/src/schema/navigator.surql` instead of copying the schema into the glossary.
