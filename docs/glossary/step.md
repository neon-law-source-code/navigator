---
title: "Step"
---

A unit of work executed by the runtime when entering a State. Each Step declares its [Actor Class](actor-class.md)
through [`workflows::step::StepKind`](../../workflows/src/step.rs), and all three are in use: `system` for the wait and
render steps (`generate_pdf`, `sent_for_signature`, `intake_persisted`), `lawyer` for the human gates (`lawyer_review`,
`filing`, `firm_signature`), and `respondent` for client-side signing. Retainer intake is the worked example — see
[`docs/retainer_intake`](../retainer_intake.md) — not the only shipped flow; the catalog of prefixes is
`workflows::step::STEP_PREFIXES` in that same module.

A `lawyer` Step means **any** lawyer-tier person in scope on the Project may advance it: the firm lens is granted by the
firm-side [Person–Project Role](personproject-role.md) row, not by the DRI marker. A narrower set of matter-level
accountability actions is reserved to the [Lawyer DRI](directly-responsible-individual-dri.md) — the `LawyerDri` viewer
in [`webapp::matter_surface`](../../webapp/src/matter_surface.rs), and the `LawyerDriRequired` refusal in
[`store::participation`](../../store/src/participation.rs). Lawyer acts; the DRI answers for it.
