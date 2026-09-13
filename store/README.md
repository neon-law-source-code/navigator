# Store

`store` is Navigator's SurrealDB data layer. It owns the schema, canonical seeds, connection configuration, and shared
data-access rules used by the web server, CLI, workflows, and Navigator MCP.

The schema lives in one file. [`src/schema/navigator.surql`](src/schema/navigator.surql) holds idempotent `DEFINE`
statements that any process applies on boot, and a `schema_version` record names the revision a database carries. The
connection contract is `NAVIGATOR_SURREAL_ENDPOINT`, `NAVIGATOR_SURREAL_NAMESPACE`, and `NAVIGATOR_SURREAL_DATABASE`.

It serves every application component that reads or writes durable domain state. A single schema owner is necessary to
keep the schema, vocabulary, authorization scope, and test data consistent across all interfaces.

Role determines an authenticated person's application tier; Project participation determines matter scope. See the
[glossary](../docs/glossary.md), [access model](../docs/access-model.md), and [test database](../docs/test-database.md).
