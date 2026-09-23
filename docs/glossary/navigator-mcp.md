---
title: "Navigator MCP"
---

The workspace's **agent surface**. Navigator MCP exposes one tool catalog through two protocol surfaces — A2A and MCP —
so clients across the ecosystem can drive Neon Law Navigator's workflows without caring which underlying LLM does the
routing.

- **A2A** (Agent2Agent) — session-gated agent card at `/app/api/mcp.json`, JSON-RPC at `/app/api/mcp/rpc`. Used by
  Gemini Enterprise and any other A2A-compatible orchestrator. A free-form `message/send` is interpreted by a pluggable
  [`AgentRouter`](../../portal/src/agent_router.rs) (Vertex AI Gemini Flash in prod) that maps the user's text to one of
  the declared tools.
- **MCP** — JSON-RPC at `/app/mcp`. Used by Claude.ai Connectors, Claude Code, LibreChat, and other Anthropic-stack
  clients. The MCP-side LLM (e.g. Claude) does its own tool routing client-side; our server just dispatches the named
  tool.

Navigator MCP is **LLM-agnostic** by design — the router behind A2A is one implementation of a trait that could be
swapped for Claude (direct or via Vertex AI Model Garden), a local model, or even a rules engine without touching the
tool catalog or the A2A wire format.

Skill names are mirrored across both protocols by [`mcp::tools::list_tools()`](../../mcp/src/tools/mod.rs): the MCP tool
name and the A2A skill id are one bare string (`create_person`). The server names itself in `serverInfo`, so a client
that flattens several MCP servers into one list groups these by the server rather than by a prefix on every tool.

- Card builder: [`portal::a2a`](../../portal/src/a2a.rs) Router trait:
  [`portal::agent_router`](../../portal/src/agent_router.rs) Tool registry: [`mcp::tools`](../../mcp/src/tools/mod.rs)
