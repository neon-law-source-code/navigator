---
title: "State"
---

One named position in a questionnaire or workflow machine. Notation rows carry the current state as a string. State
names use the `<prefix>__<discriminator>` form so the runtime can pick the right [Actor Class](actor-class.md) per
state. A **workflow** state's prefix is a step from the workflow-step catalog (`lawyer_review`, `sent_for_signature`); a
**questionnaire** state's prefix is a [Question Type](question-type.md) and its discriminator is the role
(`entity__company`), so two answers of one type stay distinct.
