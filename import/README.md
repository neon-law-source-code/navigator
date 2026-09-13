# Import

`import` is the shared engine for bulk-loading organizations, people, and their relationships into Navigator. It parses
and validates one versioned payload, then applies idempotent find-or-create writes with row-level outcomes.

It serves the CLI, web, and Navigator MCP surfaces so every contact import follows the same normalization and
deduplication rules. The shared engine is necessary to keep retries safe and prevent surface-specific interpretations of
firm data.

See [bulk contact import](../docs/bulk-contact-import.md) for the payload contract and operational behavior.
