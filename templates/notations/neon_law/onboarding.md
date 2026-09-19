---
kind: onboarding
title: Onboarding Letter
respondent_type: person_and_entity
code: onboarding__letter
jurisdiction: NV
confidential: true
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
      Which state's law governs this engagement? Nevada, unless the Firm has agreed otherwise; California and Washington
      are the alternatives available.
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

Re: Engagement for legal services — {{project__engagement.name}}

Dear {{person__client.name}}:

Thank you for engaging Shook Law PLLC ("Neon Law"). This letter sets out what we do for you, what we charge, and how
either of us ends the engagement.

We charge flat, pre-approved fees. We do not bill by the hour.

## I. Scope

**Our client is {{entity.name}}**, not its people, including you.

**The base engagement.** We act as your base attorney: you can list us publicly, we can become your registered agent in
certain jurisdictions, and you can call us in an exigent emergency.

**Litigation.** Defending a civil action brought against you, or bringing one on your behalf, for as long as the action
runs, including an appeal arising from it. Each case is engaged separately. A matter in which you already have counsel
of record is outside this engagement unless we are separately retained as co-counsel in writing. If a case reaches a
trial setting, an appeal, or class allegations, we may propose a revised fee under Section VII.

**Drafting.** A master service agreement, an employment agreement, or anything else bespoke.

**Negotiation.** Negotiating an instrument with a counterparty on your behalf to best protect your interests.

The work we are starting with:

> {{custom_text__engagement_scope}}

We advise on federal law and the law of the jurisdictions where our lawyers are admitted. Anything else is added under
Section VII.

{{custom_clauses}}

## II. Fees

These fees apply, and no others.

| Work | Fee |
| --- | --- |
| Base Membership | $50 per day |
| Litigation | $5,000 per case, per month |
| Master service agreement, drafted | $5,000 per instrument |
| Employment agreement, drafted | $1,000 per instrument |
| Negotiating with a counterparty | $500 per counterparty, per month |
| Sending a library agreement for signature | $5 per send |

Base Membership is the day rate; every other fee in the price sheet is charged in addition to it. The sheet carries no
financing line, so work on a financing round is new work, quoted and agreed under Section VII.

The day rate runs from the date of this letter until the engagement ends. We bill it monthly, for the days in the month.

A negotiation month is charged only when we work on that counterparty in it. A negotiation that goes quiet costs nothing
until it resumes. Drafting the instrument being negotiated is separate work, priced above and payable on delivery.

We require payment in advance, which we hold in our client trust account. We agree the opening deposit in writing before
we start, and you replenish it when it falls below half. We draw on it as we earn a fee or incur a cost, we apply it to
each invoice when we issue one, and we refund whatever we have not earned when the engagement ends.

## III. Costs and payment

For litigation matters, you are responsible for filing and appearance fees, service and discovery costs, court reporter
and transcript fees, expert and witness fees, and other similar charges. We do not advance them.

We invoice through Xero. Each invoice arrives by email with a secure link that takes a bank payment or a card payment. A
card payment carries a processing surcharge.

Invoices are due thirty days from the invoice date, and an invoice unpaid after that carries interest at the lesser of
one percent per month and the maximum rate the law allows. If one is unpaid thirty days past its due date, or you do not
replenish the deposit, we may suspend work on every matter under this letter and withdraw from any of them, on written
notice and subject to any approval a court requires. Suspension does not affect any billed work.

## IV. Staffing

**Your directly responsible individual is {{person__lawyer_dri.name}}**, reachable at {{person__lawyer_dri.email}} and
through our web portal. Write to contact@neonlaw.com; that address is always open to you. We may use other lawyers,
contract lawyers, paralegals, staff, and outside vendors where that suits the work. We protect your confidences as the
law and the applicable professional rules require.

## V. Response times

We respond to your requests within three business days — a business day being any day other than a Saturday, a Sunday,
or a United States federal holiday. That is a commitment to respond, not to finish, though we work to finish. It does
not run while work is suspended under Section III.

## VI. The contract library

We maintain an up-to-date library of generic and common agreements. They are unreviewed forms, not drafted for your
transaction or your counterparty. Before using them, you must make a judgment call whether they're in the best interests
of your business. If you have any questions, please contact us. Sending one for signature on your instruction is
administrative and is not our approval of it.

If you need a bespoke customization, we will draft it for you for a pre-defined fee.

## VII. Adding to the scope

We add new work by written agreement, after a conflicts check and a flat-fee quote you accept. We do not start until
then. An instruction to start work is not itself an engagement, and we may decline a matter we cannot take.

## VIII. Your responsibilities

You give us the documents and information a matter needs, complete and accurate, and tell us promptly when something
material changes — a deadline, a demand, a filing received, a counterparty's position, or a financing you are raising.
We rely on what you tell us.

Once you reasonably anticipate a dispute, you preserve the documents, messages, and other material that bear on it, and
you follow any litigation hold we send you. A court can sanction you for material destroyed after that duty attaches.

We do not guarantee any outcome. Our advice is for your use and is not to be relied on by anyone else.

## IX. Confidentiality, privilege, and technology

Our work on this engagement is confidential and, where the law provides it, privileged. Privilege is lost by disclosure
and cannot be recovered afterwards, so do not forward our advice to people who do not need it.

We use cloud storage, document management, legal research, communication, and artificial-intelligence tools in this
work, including the Neon Law portal, and by signing you consent to that. Where a vendor offers the setting, we choose
the one that keeps your data out of its generally available training models. No third-party system can be made
risk-free. A lawyer reviews what these tools produce before it becomes advice to you or goes into a filing.

When the engagement ends, we retain or return your files as the law and the applicable professional rules require, and
we destroy what remains of the file five years afterwards unless you ask otherwise.

## X. Changing rates and technology

We may raise any fee in this letter and change the technologies we use. Either way, we give you at least ninety days'
written notice first, and you may end this engagement within that period under Section XIII.

## XI. Conflicts

We have checked our records against the work described above and are aware of no conflict that prevents us from acting.
We tell you promptly if that changes.

You agree we may act for another client on a non-litigation matter unrelated to yours, including a competitor, so long
as we do not use your confidential information against you. That consent does not reach a contested proceeding against
you. On a financing we act for the company alone.

## XII. Governing law and arbitration of disputes

The law of {{custom_single_choice__governing_law}} governs this letter.

Except for a fee dispute you elect to arbitrate under a statutory fee-arbitration right that cannot be waived — in
California, the Mandatory Fee Arbitration Act; in Nevada and Washington, the fee-dispute programs of the State Bar of
Nevada and the Washington State Bar Association — any controversy or claim arising out of or relating to this engagement
or its breach, including any claim of professional negligence, malpractice, or breach of fiduciary duty, shall be
settled by final and binding arbitration administered by the American Arbitration Association under its Commercial
Arbitration Rules, before a single arbitrator, seated in {{custom_single_choice__governing_law}}, and conducted
confidentially. The AAA's rules and fee schedule govern administrative fees and arbitrator compensation, and judgment on
the award may be entered in any court of competent jurisdiction.

The arbitrator applies the same law and may award the same remedies a court would. This clause picks the forum for a
dispute; it does not limit, cap, or waive our responsibility for our own work, and it does not override a right the law
makes non-waivable.

By signing, you and we each give up the right to a jury trial and to have a covered dispute decided in court, including
a claim that we did our work negligently. Because this section governs how a future dispute between you and your lawyers
is handled, you may consult independent counsel before agreeing to it.

## XIII. Ending the engagement

Either of us may end this engagement at any time, in writing, subject to any approval a court requires to withdraw from
a pending action. If you are acquired, either of us may end it. You remain responsible for fees and costs incurred up to
that point, including the prorated day rate, the monthly fee on any case then pending, and the negotiation fee for any
month then running. You sign a substitution of attorney promptly where one is needed. On request, we return your files,
cooperate in an orderly handover to replacement counsel, and you appoint a replacement registered agent where we were
acting as yours.

Sections II, III, IX, and XII survive.

## XIV. Acceptance

Please confirm your agreement by signing below and returning a copy. Electronic signatures and counterparts are
acceptable.

Sincerely,

**Shook Law PLLC**, trading as **Neon Law**

By: {{firm.signature}}\
Date: {{firm.date}}

**Agreed and accepted:**

**{{entity.name}}**

By: {{client.signature}}\
{{person__client.name}}, for {{entity.name}}\
Date: {{client.date}}
