---
name: human-readable
description: >-
  Format any document (plan, report, doc, summary) to be human readable — concise, glanceable, and in the right medium
  for each information type (numbered lists for processes, tables for configs, rendered diagrams for graphs), with
  inline jargon explanations and direct positive statements. Use whenever writing or revising a document a human will
  read, or when the user asks to make something "human readable".
---

# Human-readable documents

Before naming a table, field, state, or UI concept, follow the **Find the term first** section in `AGENTS.md`.

Write short, glanceable documents in plain professional language. Decide the content first, then organize it. Refactor
whenever the document is updated.

## Match the medium to the information

| Information | Medium |
| --- | --- |
| A process | Short numbered list |
| A config set | Table |
| A graph — states with transitions, components with dependencies, flows that branch, loop, fork, or join | Diagram |
| Everything else, including notes and asides | Plain prose or short bullets |

Choose deliberately, by what the information is. Text always renders, and flattening a graph into a list loses the
topology the reader needs at a glance. Reserve blockquotes for actual quotations.

Draw a diagram as the type that matches the structure: state diagram, sequence diagram, dependency graph, flowchart.
Render it to PNG or SVG and embed it, since the viewer may not render Mermaid and ASCII art reads poorly. Label every
node and edge, add a short caption, and add a legend where color or shape carries meaning. Split a diagram too dense to
read at a glance.

## Structure

- Group and label related points by type (security, scalability, …) only where it helps the reader.
- Leave out scope, categories, and sections the content does not call for.
- Say each thing once. Where a table or diagram already carries it, point to that.
- To ask for review, say what to review and how — "confirm there are no unreachable states or missing transitions by
  tracing the diagram" — and leave the material where it is.
- Use plain, descriptive section titles. A question-framed heading suits experimental or research content where the
  question adds value. Split a section that answers two questions.

## Precision

- Define every coined term inline, at first use: "tree-share (the fraction of rollouts that used a tree-family
  topology)". The reader will not remember it between sessions.
- Explain jargon inline too, in the sentence, cell, or list item that uses it. Keep definitions with the term.
- State scope exactly: "all of X", or "a sample of 200 from X".
- Give numbers: "~3s" over "fast", "12 of 40" over "some".
- Name the specific thing: "the checkout retry loop" over "some code paths".
- For each data source, say what it includes and what it excludes. For a sample spanning several sources, say how it
  is distributed across them.
- Ground evidence in references, in a References section at the end.

## Sentence-level style

Two house voices, applied together.

**Apple** — one idea per sentence. Put the verb early. Name the thing, then say what it does. Cut hedges: somewhat,
fairly, generally, it is worth noting, it should be said.

**Cravath** — state the rule first and its reason after, in the same sentence or the next one. Attribute a claim to its
source. Cut the archaeology: give the rule in force, without narrating the rule it replaced. A document that explains
each of its own revisions is a history of itself, and the reader needs the version that governs.

Cut the history only where it is decoration. Keep a prior state the reader acts on — a migration path, a deprecation
with a date, a decision whose reason still binds.

## Direct, positive statements

- Say what something is. State the surviving claim as one direct sentence and delete the rejected half: no "not X,
  but Y", "it isn't X, it's Y", "not only X", "less X, more Y", and no pivots through actually, instead, rather, really,
  the real, or what matters.
- Keep headings plain. Avoid reframe headings ("What actually matters") and rhetorical questions.
- Use contrast only to correct a specific factual error — a date, number, name, or scope.
