---
kind: onboarding
title: Engagement Letter
respondent_type: person_and_entity
code: onboarding__engagement_letter
jurisdiction: NV
confidential: true
output: letter
prompts:
  client_name: Who is the Client's directly responsible individual, the one person the Firm takes instructions from?
  project_name: What is the project name for this engagement?
  lawyer_dri: Which lawyer is directly responsible for this engagement?
audiences:
  client_name: client
  project_name: lawyer
  governing_law: lawyer
  lawyer_dri: lawyer
  engagement_start_date: lawyer
  engagement_scope: lawyer
  entity: lawyer
  principal_office: lawyer
custom_questions:
  engagement_scope:
    prompt: >-
      In a sentence or two, what is the minimum scope of this engagement.
  engagement_start_date:
    prompt: When does this engagement begin?
  governing_law:
    prompt: >-
      Which state's law governs this engagement? Nevada by default; choose other state we practice in if
      the Client is located there.
    choices:
      nevada: Nevada
      california: California
      washington: Washington
questionnaire:
  BEGIN:
    _: entity
  entity:
    _: address__principal_office
  address__principal_office:
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

{{entity.name}}\
{{address__principal_office}}

Attn: {{person__client.name}}

Re: Engagement to Provide Legal Services — {{project__engagement.name}}

Dear {{person__client.name}}:

Thank you for engaging the Firm. This letter (the "Engagement Letter") sets out the terms on which the Firm will
represent {{entity.name}} (the "Client" or "you") in the matter described below. This
says what we are doing now, who is accountable on each side, how you reach us, how we bill, and how we resolve a
disagreement if one arises. Everything particular to your matter is agreed in writing as we go, and each of those
writings sits on top of this letter rather than replacing it.

If these terms are acceptable, please sign below and return a copy.

## I. Client and scope of the engagement

For this engagement the Firm's client is {{entity.name}}. Unless the Firm agrees in a separate signed writing,
this engagement does not make any affiliate, stockholder, investor, officer, director, employee, or other related person
or entity a client of the Firm.

The Firm will represent the Client in the following matter (the "Matter"):

> {{custom_text__engagement_scope}}

**That is the floor, not the ceiling.** The Firm's representation is limited to the work described above and anything
the Firm and the Client later agree to in writing. Work outside it — a new matter, a dispute, a proceeding, an appeal —
requires a separate written engagement or a written amendment to this one signed by both of us. We would rather add
scope in a two-line email exchange than have you assume we are already handling something we are not.

This letter serves the Matter whether it is transactional or litigation. **Where the Matter is transactional**, the
scope covers the drafting, negotiation, review, and counseling the description above calls for. **Where the Matter is a
dispute**, the scope covers the ordinary work of prosecuting or defending it as described above — strategy, pleadings,
discovery, ordinary motion practice, settlement negotiation, and coordination with any co-counsel or vendors the Client
authorizes — and the fee writing described in Section II states which litigation events, such as an evidentiary-hearing
day or a trial day, carry their own fee.

Unless separately agreed in writing, this engagement does not include tax, accounting, financial, investment, valuation,
insurance-coverage, or public-relations advice, and does not extend to a matter unrelated to the Matter described above.

{{custom_clauses}}

## II. Fees, costs, and invoices

Fees for this engagement are set in a writing the Firm and the Client agree to — a flat monthly fee, an hourly rate, a
contingency, per-day fees for named litigation events such as an evidentiary hearing or a trial day, or a combination —
and that writing controls the fee. **The Firm will not begin work before that writing is signed**, so you always know
the basis on which you are being charged before anything is billed. Where a fee is contingent on a recovery, the rate is
not set by law and is negotiable between the Firm and the Client, and the contingency is written out in its own signed
fee agreement stating the percentage, how the recovery is defined and the fee calculated — including how consideration
received other than in cash is handled — and how litigation costs affect what you ultimately owe. Advance fees are
handled under the applicable client-trust rules, and any unearned portion is refundable if the representation ends or
the agreed services are not completed.

> **A. Costs.** Fees do not include filing fees, expert fees, mediator fees, court reporter and transcript costs,
> e-discovery and vendor costs, travel expenses, or other third-party costs. Those are passed through at cost and are
> the Client's responsibility when incurred with the Client's authorization or reasonably necessary for the engagement.
> The Firm seeks advance approval for a material outside cost when that is practical.
>
> **B. Every invoice carries its own payment instructions.** The Firm invoices by email, and the instructions for paying
> an invoice are printed on that invoice. Read them there rather than reusing instructions from an earlier invoice,
> because they can legitimately change from one invoice to the next. Before acting on any change in payment instructions
> that reaches you by email — including a message that appears to come from the Firm — verify it by telephone with your
> lawyer at a number you already know to be genuine. The Firm will never be annoyed by that call.

If the work required falls outside the agreed scope, the Firm and you will discuss the additional scope and the fee
arrangement before the Firm undertakes that work.

## III. Staffing, and who is accountable on each side

Every matter the Firm opens names **one person on each side who answers for it** — one lawyer here, one person at the
Client. We call each of them the directly responsible individual, or DRI. Other people work on the matter; these two
answer for it, so you always know who to ask where things stand. Neither name changes except in writing.

> **A. The Firm's directly responsible individual is {{person__lawyer_dri.name}}**, reachable at
> {{person__lawyer_dri.email}}. That lawyer is principally responsible for this engagement — for the work, for the
> schedule, and for telling you candidly where the Matter stands.
>
> **B. The Client's directly responsible individual is {{person__client.name}}.** The Client DRI is the person the
> Firm takes instructions from, asks when a decision is needed, and sends advice to on the Client's behalf. Where the
> Client is an organization, the Client DRI speaks for the organization on this Matter and the Firm may rely on that.
> Other people at the Client may sign this letter or be copied on the work; the Client DRI is who we call.

The Firm may use lawyers, contract lawyers, paralegals, administrative personnel, or outside vendors where that is
appropriate, and remains responsible for their work and for protecting your confidences as law and the applicable
professional rules require.

## IV. Reaching us, and reading your own files

> **A. Write to contact@neonlaw.com.** That address is always open to you, for anything, at any point in the
> engagement — a question, a document, a complaint about how the Matter is going, or a request for a status update. It
> reaches the Firm rather than one inbox, so it does not go stale when someone is in a hearing or on leave. Writing to
> your lawyer directly is fine too; contact@neonlaw.com is the address that always works.
>
> **B. Your documents are available to you at www.neonlaw.com.** Sign in there and you can read and download the
> documents the Firm has shared with you on this Matter — the letters, the agreements, and the filings — for the life of
> the engagement. Not everything in the Firm's file is posted there: our internal notes and drafts are working papers,
> so the portal is a convenience and not the file itself. **You may ask for a copy of anything in your file at any time,
> without explaining why**, and Section VII says how long the Firm keeps it.

You consent to electronic communication, and to electronic delivery of invoices and correspondence about the Matter, at
the addresses the Client DRI gives the Firm.

## V. Conflicts, other clients, and advance waiver

The Firm represents and may in the future represent other clients. If a potential conflict arises, the Firm addresses it
under the applicable rules of professional conduct. Unless you are accepted as a client in a specific additional matter
by a signed writing, the Firm is not agreeing to represent you in every matter or against every potential adverse party.

The Firm treats a conflict for any one of its lawyers as a conflict for the whole firm. Before taking on a new matter we
check it against our current and former matters. If that check turns up a conflict we cannot properly take on, we tell
you promptly, decline the matter rather than wall it off internally, refer you to outside counsel, and return any
materials you shared with us. The Firm neither pays nor accepts a referral fee on a matter it refers out. By engaging us
you acknowledge that our lawyers share matter information among themselves for this purpose.

> **A. Advance waiver — transactional matters only.** You agree that the Firm may represent other clients — including
> clients whose interests are adverse to you or your affiliates — in transactional, corporate, commercial, licensing,
> regulatory, counseling, and other non-litigation matters, provided that (i) the matter is not substantially related to
> this engagement or the Matter, and (ii) the Firm protects your confidential information as the applicable professional
> rules require. By signing this letter you give informed written consent to those transactional representations, and
> you acknowledge that you have had the opportunity to consult independent counsel about this waiver.
>
> **B. No litigation waiver.** This advance waiver is limited to transactional and other non-litigation matters. You do
> not waive, and the Firm does not request, advance consent for litigation, arbitration, or any other contested
> adversarial proceeding. The Firm will not appear adverse to you in any litigation, arbitration, or contested
> proceeding while it represents you unless you separately give informed written consent at that time under the
> applicable rules of professional conduct.

## VI. What we each do

You agree to provide accurate and complete information, respond to reasonable requests, and make the decisions the
representation needs. Where the Matter is in litigation or a dispute is reasonably anticipated, you also agree to
preserve potentially relevant documents and information, and to appear for depositions and other proceedings the Matter
requires. The Client agrees to pay fees and authorized expenses when due. The Firm's advice depends on the information
available to it when the advice is given.

The Firm has not made and cannot make any promise, assurance, or guarantee about the outcome of any matter, negotiation,
proceeding, settlement, or business objective.

## VII. Confidentiality, your file, and technology

The Firm maintains your confidences as law and the applicable professional rules require. That duty does not vary with
the kind of Matter: what you tell us in a negotiation is held as closely as what you tell us in a dispute.

The Firm may use secure cloud, document-management, research, communication, automation, and artificial-intelligence
tools in providing legal services, subject to its professional obligations, attorney supervision, and commercially
reasonable security and confidentiality safeguards. Where a technology vendor offers the option, the Firm selects
settings that do not permit your information to be used to train the vendor's public or generally available models. No
AI output substitutes for counsel's professional judgment: a lawyer reviews material AI-assisted work before it is
relied on for legal advice, a filing, or a substantive external communication. The Firm remains responsible for the
accuracy, confidentiality, and professional review of its work. Your consent to that use waives no privilege and does
not release the Firm from responsibility for selecting, configuring, supervising, or using the technology.

The Firm keeps your complete matter file — every document, signed agreement, and the privileged correspondence we
exchange with you — for ten years after your matter closes. You may request a copy at any point in that period. After
ten years the Firm securely destroys the file and its contents.

## VIII. Governing law, and arbitration of disputes

This letter is governed by the law of {{custom_single_choice__governing_law}}. If a dispute arises out of or relates to
this engagement or this letter, you and the Firm agree to resolve it by final and binding arbitration before a single
arbitrator administered by **JAMS** under its Comprehensive Arbitration Rules and Procedures — or, where the amount in
controversy qualifies, its Streamlined Rules — conducted confidentially and decided under the law of
{{custom_single_choice__governing_law}}. Each party bears its share of the JAMS fees as those rules provide, and
judgment on the award may be entered in any court of competent jurisdiction. The arbitrator applies the same law and may
award the same remedies a court could; this clause selects the forum for a dispute and does **not** limit, cap, or waive
the Firm's responsibility for its own work.

> **A. Your fee-arbitration rights are preserved.** Nothing above waives or overrides any non-waivable statutory right
> you have to arbitration of a fee dispute — including, in California, the Mandatory Fee Arbitration Act (Bus. & Prof.
> Code § 6200 et seq.), and the corresponding fee-dispute programs of the State Bar of Nevada and the Washington State
> Bar Association. You keep those rights in full.

By signing this letter you and the Firm each give up the right to a jury trial and to have a covered dispute decided in
court. Because this is an agreement about how future disputes are handled, you have the right to consult independent
counsel of your own choosing before you agree to it.

## IX. Ending the engagement

You may end this engagement at any time by telling us. The Firm may withdraw as law and the applicable professional
rules permit or require — including for nonpayment, a conflict, a failure to cooperate, or other good cause — and
subject to the rules governing a lawyer's withdrawal from a pending matter. Fees and authorized expenses incurred before
that point remain due.

## X. Signatures

The Client and the Firm sign this letter electronically as of the dates below.

{{client.signature}}

{{client.date}}

By initialing here, the Client acknowledges that this engagement covers the scope described in Section I and nothing
else, and that additional work requires a separate written engagement or a written amendment signed by both of us:
{{client.initials}}

{{firm.signature}}

{{firm.date}}
