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

To add a term, create `<slug>.md` whose name is the slug of its `title:` (`Lawyer Review` → `lawyer-review.md`), link
other entries as siblings (`[Matter](matter.md)`), and run `navigator glossary tables --write` if it names a table.
