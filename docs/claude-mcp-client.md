# Claude as an AIDA client

How to point **Claude** — Claude Code or Claude Desktop — at a running Neon Law Navigator deployment, so an attorney can
open a matter with its entities and people by asking for it in plain English.

This is the **setup and capability** story. The runtime interaction model — where AIDA pauses, how a failure's reason
reaches the user — is [`aida-a2a-interaction.md`](aida-a2a-interaction.md). The Gemini Enterprise equivalent, which
dials `/mcp` over HTTPS instead, is [`gemini-enterprise-mcp.md`](gemini-enterprise-mcp.md).

## What runs where

Claude speaks MCP: local stdio servers and remote HTTP connectors. It has no A2A client. So `navigator site mcp` is the
adapter — MCP on stdio facing Claude, A2A facing the deployment:

```text
Claude Code / Claude Desktop
   │   MCP JSON-RPC 2.0, newline-delimited, over stdio
   ▼
navigator site mcp            ← runs on the attorney's laptop
   │   POST /app/api/aida/rpc
   │   metadata.skill = the tool Claude chose
   │   Authorization: Bearer <the 1h token `site login` stored>
   ▼
web  →  portal::a2a::dispatch_single  →  mcp::tools::call_tool
        lawyer-tier check + target: "audit" events
```

The credential is a first-party one. `inject_bearer_session` resolves the signed `SessionData` blob on this route,
`require_google_oauth` lets an already-resolved first-party session past its tokeninfo check rather than rejecting it
for not being a Google token, and `inject_principal` reads the principal off the session's `email`. All three are
needed: without them the CLI's credential reaches the endpoint and is redirected to a login page a JSON-RPC client
cannot follow. None of it widens who may call — the same Rego lawyer-gate still decides, from the role on that session.

Two more decisions are worth stating, because both are load-bearing.

**It dials A2A, not `/mcp`, even though it speaks MCP to Claude.** A2A is where the supervision lives: the lawyer-tier
check, and the `target: "audit"` record of every decision. Sending to `/mcp` would be one fewer hop and would skip all
of it.

**Claude picks the tool, not Gemini.** A2A's free-form path runs its own agentic loop with Vertex AI choosing the tools.
Bridging that would put two models in series and let the weaker one decide the actions. The bridge sends
`metadata.skill` instead, so the tool and its arguments are Claude's decision and the deployment's job is to authorize
and execute.

## Setup

### 1. Log in to the deployment

```bash
navigator site login --host www.neonlaw.com
```

Browser-loopback flow; the token lands at `~/.navigator.json`, mode `0600`, and lasts one hour. The account must resolve
to a `persons` row carrying the `lawyer` or `admin` role — the tools that write refuse anything less. Check it with
`navigator site whoami`.

### 2. Register the server

Claude Code:

```bash
claude mcp add navigator -- navigator site mcp --host www.neonlaw.com
```

Claude Desktop, in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "navigator": {
      "command": "navigator",
      "args": ["site", "mcp", "--host", "www.neonlaw.com"]
    }
  }
}
```

`--host` is optional when one host is logged in. For a local cluster pass an absolute URL — the port is in that
worktree's `.devx/env`:

```bash
navigator site mcp --host http://localhost:3001
```

The server speaks protocol on stdout and diagnostics on stderr, because the client parses every stdout line. Run it from
a client configuration, not by hand: started in a terminal it waits on stdin and looks hung.

## What Claude can and cannot do

The catalog Claude sees is deliberately narrower than the deployment's — eleven tools of the fourteen. Every read, plus
the writes that touch only the firm's own records:

| Offered | Why |
| --- | --- |
| all 7 reads | a lookup changes nothing, and answers through the caller's own lens |
| `aida_create_person` | a contact row, visible and correctable in `/app/admin` |
| `aida_create_project` | opens a matter, on the attorney's own conflict attestation |
| `aida_link_person_project` | participation on a matter |
| `aida_bulk_import` | a whole contacts document — organizations, people, and the links |

| Withheld | Why |
| --- | --- |
| `aida_send_welcome_email` | it emails a client |
| `aida_create_notation` | a Notation is a binding legal artifact |
| `aida_answer_notation` | it answers one |

The withheld three need a lawyer's explicit approval, which is an `input-required` pause on the A2A side. **MCP has no
way to pause a call and ask a person.** A two-call handshake would not fix that: if Claude makes both calls, the model
is approving its own action, which is not supervision. So rather than simulate a gate this transport cannot carry, those
tools are absent from `tools/list` — Claude cannot see them, so it never proposes an act that could not be supervised.
Naming one anyway is refused by the bridge without any dispatch, with a result saying to do it in the app instead.

Do those three in `/app`, where a human approves in a UI and the approval is recorded against the matter.

Confirmation is the only reason anything is withheld. `aida_list_projects` is offered because the read answers through
the caller's own lens, so there is nothing left to withhold.

Which lens depends on the tier, and the two answer different questions, so they return different shapes. The `lens`
field on the response says which one came back.

| Tier | Lens | What comes back | Predicate |
| --- | --- | --- | --- |
| `lawyer`, `clerk`, `client` | `membership` | the matters the caller is on, with `id` | `access::visible_projects` |
| `owner`, `admin` | `directory` | `code`, `name`, `status`, `lawyer_dris` | `projects::matter_directory` |

The membership lens is the same predicate `/app/projects` renders from, and it carries the `id` the write tools take.

Owner and Admin are not invited to matters, so they hold no participation row and the membership lens would correctly
show them nothing. The directory lens is oversight instead: it answers which matters exist and who is accountable for
each, and carries nothing a matter *contains*. There is no `id` on it deliberately — `code` is the stable handle, and
anything a matter holds is membership's to disclose. A `lawyer` does not get this lens.

An authenticated email with no `persons` row lists nothing, and neither does a caller whose tier holds no matching
participation row. Sign-in does not create a Person, so a stranger with a valid token is exactly the caller who must
reach nothing.

`aida_show_person` searches the firm's own people directory, so it is firm-side: `owner`, `admin`, and `lawyer` read it,
while `clerk` and `client` are refused. A refusal rather than an empty list, because nothing the model could type would
make the read succeed, and "no matches" would teach it the directory is empty.

## Opening a matter end to end

Everything the onboarding chain needs is in the offered set, so this is one conversation:

1. **The organization and its people.** Hand Claude the contact list and ask it to load them. It calls
   `aida_bulk_import` with one document; find-or-create means a re-run changes nothing. The payload contract is
   [`bulk-contact-import.md`](bulk-contact-import.md).
2. **The matter.** `aida_create_project` against the entity that import created. It needs a `code` — the stem of the
   matter's code, later the base of its repository name. Navigator appends a short generated suffix, since a code is
   chosen once at matter-open and never changes; give Claude a preferred stem rather than a final code. It also requires
   the opening attorney's conflict `attestation`, and refuses without it.
3. **Who is on it.** `aida_link_person_project` per participant.

`aida_create_project` provisions the matter's repository itself, best-effort:
`store::project_surfaces::reconcile_after_open` runs right after the matter opens and, when forge credentials are
configured, creates or adopts the GitHub repository named for the code and records its URL on `projects.repository_url`.
Missing forge configuration skips that step rather than failing the open, so an unconfigured deployment still records
the coordinate with no repository behind it yet. An operator reconciles later:

```bash
navigator site projects surfaces reconcile --project <code>
```

## Limits, stated rather than discovered

- **stdio is per-laptop.** This serves Claude Code and Claude Desktop. It does not serve claude.ai on the web or mobile,
  which would need OAuth protected-resource metadata and dynamic client registration.
- **The token lasts one hour** (`CLI_SESSION_TTL_SECS`). When it ages out, calls come back as a tool error. Run
  `navigator site login` again; the credential is read from disk per call, so re-login lands without restarting the
  server or the client.
- **Registering before logging in is fine.** The server starts, lists its tools, and explains the missing login on the
  first call. It does not refuse to start: a stdio server that exits has no way to tell the client why, so the client
  would show a dead entry and no reason.
- **The advertised catalog is compiled into the CLI.** A `navigator` older than the deployment advertises the catalog it
  shipped with. Both directions fail gracefully: a tool the deployment has but the binary does not is simply
  unavailable, and one the binary offers that the deployment has dropped returns an unknown-tool error naming it.
- **Every call is audited.** A write dispatched this way emits the same `a2a.direct_skill.side_effect` event, with the
  authenticated email, that any other A2A client's would.
