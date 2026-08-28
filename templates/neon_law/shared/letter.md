---
kind: onboarding
title: Retainer Agreement
respondent_type: person_and_entity
code: onboarding__letter
jurisdiction: NV
confidential: true
output: letter
prompts:
  client_name: What is the client's full legal name?
  project_name: What is the project name for this engagement?
  lawyer_dri: Which lawyer is directly responsible for this engagement?
audiences:
  client_name: client
  project_name: lawyer
  governing_law: lawyer
  lawyer_dri: lawyer
  engagement_start_date: lawyer
  engagement_scope: lawyer
  fee_basis: lawyer
custom_questions:
  engagement_start_date:
    prompt: When does this engagement begin?
  engagement_scope:
    prompt: >-
      In a sentence or two, what is the minimum scope of this engagement - the work the Firm is committing to
      right now? Everything else is added later in writing.
  fee_basis:
    prompt: >-
      How is the Firm paid on this engagement? State the amount, the unit, and the basis, in the words of the
      writing the parties signed - for example "$450 per hour", "$12,500 per month", or "30% of net recovery".
  governing_law:
    prompt: >-
      Which state's law governs this engagement? Nevada by default; choose California or Washington only if the
      Client is located there.
    choices:
      nevada: Nevada
      california: California
      washington: Washington
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: person__lawyer_dri
  person__lawyer_dri:
    _: project__engagement
  project__engagement:
    _: custom_datetime__engagement_start_date
  custom_datetime__engagement_start_date:
    _: custom_text__engagement_scope
  custom_text__engagement_scope:
    _: custom_text__fee_basis
  custom_text__fee_basis:
    _: custom_single_choice__governing_law
  custom_single_choice__governing_law:
    _: END
  END: {}
workflow:
  BEGIN:
    intake_submitted: intake_persisted__client
  intake_persisted__client:
    retainer_rendered: lawyer_review
  lawyer_review:
    approved: generate_pdf__retainer_pdf
    changes_requested: reask__client
    rejected: END
  reask__client:
    intake_resubmitted: lawyer_review
  generate_pdf__retainer_pdf:
    pdf_persisted: sent_for_signature__pending
  sent_for_signature__pending:
    signature_received: END
    signature_declined: END
  END: {}
---

{{custom_datetime__engagement_start_date}}

{{person__client.name}}

Re: Retainer Agreement — {{project__engagement.name}}

Dear {{person__client.name}}:

This Retainer Agreement (the "Agreement") is entered into between the Firm and {{person__client.name}} (the
"Client"), reachable at {{person__client.email}}, for legal services on the matter referred to as
{{project__engagement.name}}.

## I. Scope of the engagement

The Firm will represent the Client in the following matter (the "Matter"):

> {{custom_text__engagement_scope}}

The Firm's representation is limited to the work described above, the clauses of this Agreement, and anything the Firm
and the Client later agree to in writing. Work outside that scope — including any new matter, dispute, or proceeding —
requires a separate written engagement or a written amendment to this one signed by both the Client and the Firm.

{{custom_clauses}}

## II. Fees, costs, and invoices

The Firm is paid on this engagement as follows:

> {{custom_text__fee_basis}}

Expenses are passed through at cost. The Firm seeks advance approval for a material outside cost when that is
practical.

> **A. Every invoice carries its own payment instructions.** The Firm invoices by email, and the instructions for
> paying an invoice are printed on that invoice. Read them there rather than reusing instructions from an earlier
> invoice, because they can legitimately change from one invoice to the next. Before acting on any change in payment
> instructions that reaches you by email — including a message that appears to come from the Firm — verify it by
> telephone with your lawyer at a number you already know to be genuine. The Firm will never be annoyed by that call.

## III. Who is accountable on each side

Every matter the Firm opens names **one person on each side who answers for it** — one lawyer here, one person at the
Client. We call each of them the directly responsible individual, or DRI. Other people work on the matter; these two
answer for it, so you always know who to ask where things stand. Neither name changes except in writing.

> **A. The Firm's directly responsible individual is {{person__lawyer_dri.name}}.** That lawyer is principally
> responsible for this engagement — for the work, for the schedule, and for telling you candidly where the Matter
> stands, and is reached through the firm channel in Section IV rather than a personal inbox.
>
> **B. The Client's directly responsible individual is {{person__client.name}}.** The Client DRI is the person the
> Firm takes instructions from, asks when a decision is needed, and sends advice to on the Client's behalf.

## IV. Reaching us, and reading your own documents

> **A. Write to contact@neonlaw.com.** That address is always open to you, for anything, at any point in the
> engagement — a question, a document, a complaint about how the Matter is going, or a request for a status update. It
> reaches the Firm rather than one inbox, so it does not go stale when someone is in a hearing or on leave.
>
> **B. Your documents are available to you at www.neonlaw.com.** Sign in there and you can read and download the
> documents the Firm has shared with you on this Matter for the life of the engagement. Not everything in the Firm's
> file is posted there: our internal notes and drafts are working papers, so the portal is a convenience and not the
> file itself.

The Client acknowledges receipt of the Firm's privacy notice and agrees to electronic delivery of invoices and
correspondence about the Matter at {{person__client.email}}.

## V. Conflicts

The Firm is a small firm, and we treat a conflict for any one of our lawyers as a conflict for the entire firm. Before
we take on a new matter, we check it against all of our current and former matters across every lawyer here. If that
check turns up a conflict we cannot properly take on — for example, where the matter would have the Firm representing
a business and an individual whose interests are adverse to each other, or would place the Firm adverse to a current
or former client — we will tell you promptly, decline the matter rather than wall it off internally, refer you to
outside counsel, and return any materials you shared with us. The Firm neither pays nor accepts a referral fee on any
matter it refers out. By engaging us, you acknowledge that our lawyers share matter information among themselves for
this purpose.

## VI. Your file, kept for ten years

The Firm keeps your complete matter file — every document, signed agreement, and the privileged correspondence we
exchange with you — for ten years after your matter closes. You may request a copy of your file at any point during
that period. After ten years, the Firm securely destroys the file and its contents.

## VII. Governing law, dispute resolution, and ending the engagement

This Agreement is governed by the law of {{custom_single_choice__governing_law}}.

**Resolving a dispute — binding arbitration.** If a dispute arises out of or relates to this engagement or this
Agreement, you and the Firm agree to resolve it by binding arbitration administered by **JAMS** under its Comprehensive
Arbitration Rules & Procedures — or, where the amount in controversy is small enough to qualify, its Streamlined Rules.
The arbitration is seated in **Reno, Nevada**, **conducted confidentially**, and decided under
{{custom_single_choice__governing_law}} law; each party bears its share of the JAMS fees as those rules provide. By
agreeing to arbitration, you and the Firm give up the right to a jury trial and to have the dispute decided in court —
except as stated in the next paragraph. The arbitrator applies the same law and may award the same remedies a court
could; this clause selects the forum for a dispute and does **not** limit, cap, or waive the Firm's responsibility for
its own work. Because this is an agreement about how future disputes are handled, you have the right to consult
independent counsel of your own choosing before you agree to it.

**Your fee-arbitration rights are preserved.** Nothing in the arbitration clause waives or overrides any non-waivable
statutory right you have to arbitration of a fee dispute — including, in California, the Mandatory Fee Arbitration Act
(Bus. & Prof. Code § 6200 et seq.), and the corresponding fee-dispute programs of the State Bar of Nevada and the
Washington State Bar Association. You keep those rights in full.

Either party may terminate this Agreement upon written notice, subject to the rules governing a lawyer's withdrawal
from a pending matter. The Client remains responsible for fees and expenses incurred prior to termination.

## VIII. Signatures

The Client and the Firm execute this Agreement electronically as of the dates signed below.

{{client.signature}}

{{client.date}}

By initialing here, the Client acknowledges that this engagement covers the work described in Section I and the clauses
of this Agreement, and that a separate matter, an appeal, or a new proceeding requires a separate written engagement
with the Firm or a referral to outside counsel: {{client.initials}}

{{firm.signature}}

{{firm.date}}
