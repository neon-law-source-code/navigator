---
title: "Question Type"
---

The `<type>` half of a questionnaire [State](state.md) name (`<type>__<role>`) — a closed set defined by
[`store::question_registry::QuestionType`](../../store/src/question_registry.rs). Each type is a **record** (creates or
links a `store::entity` row — `person`, `entity`, `address`, …), a **reference** (selects a seeded row — `jurisdiction`,
`entity_type`, `project`, …), or a **custom** primitive (`custom_text`, `custom_single_choice`, `custom_datetime`, …).
Record and reference types pair a singular with an explicit plural/aggregate (`person`→`people`). `N113` grounds every
typed state to this registry, `N114` orders `__for_` children after their parent, `N115` resolves body paths and
iterators against the declared states while `N120` does the same for a bare `{{type__role}}` token, `N117` requires
every `custom_text__*` role to be an allowlisted free-text primitive (glossary nouns — names, emails, countries, phone
numbers — can never be allowlisted), and `N118` requires the block to be one linear `_` chain from `BEGIN` to `END` (the
walker's only signal). Use the glossary model and its dotted fields — `person__client` plus `{{person__client.email}}` —
before reaching for a custom primitive. See [`notation-authoring`](../notation-authoring.md).
