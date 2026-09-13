# Navigator MCP tool catalog

`mcp` defines Navigator MCP's shared catalog of tools for working with Navigator data and legal-service workflows. The
same catalog is exposed through MCP and A2A, allowing compatible agent hosts to look up records, prepare work, and
advance authorized flows without inventing a separate integration for each model.

It serves lawyers using Navigator MCP and engineers integrating trusted agent clients. The catalog is necessary to keep
tool semantics, confirmation requirements, authorization, and audit behavior consistent with the portal and CLI.

Navigator MCP remains model-agnostic, and client-facing or mutating actions preserve their human confirmation gates. See
[Navigator MCP interaction](../docs/mcp-a2a-interaction.md) and [Gemini Enterprise
integration](../docs/gemini-enterprise-mcp.md).
