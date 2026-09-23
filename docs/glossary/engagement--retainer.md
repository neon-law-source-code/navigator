---
title: "Engagement / Retainer"
---

Client-English synonym for **[Notation](../notation.md#notation) bound to a Project**. An Engagement is what the firm
sells; under the hood, running an Engagement means creating a Notation, walking its Questionnaire, advancing its
Workflow, and rendering its document.

The **engagement is a matter's first Notation**: a [Project](project.md) is opened first (its own step — `navigator
project create` — which seeds the client and lawyer participation), then the engagement is created on it like any other
Notation (`navigator site notation create <retainer_code> --project <code>`). Opening a Project never opens a Notation
with it; no door creates a retainer alongside the matter.

The engagement-first rule is enforced at create time on the **self-serve doors** — `web`'s project-scoped create route
and the CLI, which share `workflows::notation_session::create_notation_from_repo`. There, a matter's first Notation must
be a template whose declared `kind` opens a matter — see [Onboarding](onboarding.md). One classifier answers that for
the whole workspace, `rules::kind::Kind::opens_a_matter`; see [`docs/frontmatter`](../frontmatter.md) for the `kind`
vocabulary. Later Notations — filings, letters — may be any kind.

**[Navigator MCP](navigator-mcp.md) is not bound by that rule**, because it is lawyer-directed rather than self-serve.
`create_notation` opens the notation through the policy-free `start_notation` primitive, so an attorney driving the
agent may bind a filing or letter as a matter's first Notation; gating the agent door would forbid the agent's ordinary
use. What constrains Navigator MCP is authorization, not kind: the actor must be lawyer and in scope for the Project
(`store::projects::can_access_as_lawyer_in_surreal`), and the respondent is always the Project's client-side DRI.

A **Retainer** is the same idea, narrowed: an Engagement whose bound Template is the firm's onboarding letter,
`onboarding__letter`. The `portal::retainer_walk` walker, the [`docs/retainer_intake`](../retainer_intake.md) state
machine, and the firm's "signed retainer" disclaimer all refer to that specific kind of Notation.

The schema noun in both cases is `Notation`. Client-facing copy speaks Engagement and Retainer because clients do; the
database and the workflow runtime speak Notation.
