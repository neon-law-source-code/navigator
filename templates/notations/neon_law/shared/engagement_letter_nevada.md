---
kind: letter
title: Engagement Letter — Arbitration (Nevada)
jurisdiction: NV
respondent_type: person
code: engagement_letter__nevada
confidential: false
output: letter
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: person__lawyer_dri
  person__lawyer_dri:
    _: custom_datetime__engagement_start_date
  custom_datetime__engagement_start_date:
    _: custom_text__engagement_scope
  custom_text__engagement_scope:
    _: person__adverse_party
  person__adverse_party:
    _: custom_single_choice__arbitration_forum
  custom_single_choice__arbitration_forum:
    _: custom_single_choice__governing_law
  custom_single_choice__governing_law:
    _: END
  END: {}
custom_questions:
  engagement_start_date:
    prompt: When does this engagement begin?
  engagement_scope:
    prompt: >-
      In a sentence or two, what is the minimum scope of this engagement — the
      work the Firm is committing to right now? Everything else is added later
      in writing.
  arbitration_forum:
    prompt: Which administrator hears the arbitration, and where is it seated?
    choices:
      jams: JAMS, seated in Las Vegas, Nevada
      aaa: The American Arbitration Association, seated in Las Vegas, Nevada
  governing_law:
    prompt: >-
      Which state's law governs this engagement? Nevada by default; choose
      California or Washington only if the client is located there.
    choices:
      nevada: Nevada
      california: California
      washington: Washington
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

# ENGAGEMENT LETTER

__FIXTURE DOCUMENT.__ This is a sample rendered from a notation template in the
Navigator sample project. _Cruller v. Prine_ is a simulated matter, no person
named here exists, no lawyer is engaged by anyone on the strength of this page,
and nothing in it is legal advice.

__Date:__ {{custom_datetime__engagement_start_date}}

__To:__ {{person__client}}

__Re:__ Engagement to provide legal services — Cruller v. Prine

Dear {{person__client}}:

Thank you for engaging the Firm. This letter sets out the terms on which the
Firm will represent you. It is deliberately short. It says what we are doing
now, who answers for it on each side, how we bill, where your dispute will be
heard, and how you and the Firm would resolve a disagreement between
ourselves if one arose.

If these terms are acceptable, please sign below and return a copy.

## 1. Client and scope

For this engagement the Firm's client is {{person__client}}, a natural person.
No member of the client's household is a client of the Firm by virtue of this
letter.

The Firm will represent you in the following matter (the "Matter"):

> {{custom_text__engagement_scope}}

__That is the floor, not the ceiling.__ Work outside the Matter — a new
dispute, a separate proceeding, an appeal — requires a separate written
engagement or a written amendment to this one signed by both of us. We would
rather add scope in a two-line email exchange than have you assume we are
already handling something we are not.

Unless separately agreed in writing, this engagement does not include tax,
accounting, insurance-coverage, or public-relations advice, and does not
extend to any matter unrelated to the Matter described above.

## 2. The Matter is heard in arbitration

The claims described above are resolved by binding arbitration rather than by a
jury, because the recorded declaration of covenants for the parties' street
requires the neighbors to arbitrate a dispute of this kind before either of
them may try it. The arbitration is administered by
{{custom_single_choice__arbitration_forum}}, before a single arbitrator, and conducted
confidentially.

Three consequences are worth stating plainly before you sign, because they are
the ones clients are most often surprised by afterwards.

* __No jury, and no public courtroom.__ One arbitrator decides both what
  happened and what the law requires of it. The hearing is private and the
  award is not a public record.

* __Discovery is narrower than in court.__ The arbitrator sets what exchange of
  documents and testimony is proportionate. Expect less of it than a court
  action would allow, which cuts both ways: it lowers the cost and it limits
  what can be extracted from {{person__adverse_party}}.

* __An award is nearly final.__ A court may enter judgment on the award, and may
  set one aside only on the narrow statutory grounds. A mistake about the facts,
  or about the law, is generally not one of them.

The Firm will advise you before any deadline that would waive a right, and will
not agree to consolidate, bifurcate, or submit any claim on the papers without
telling you first.

## 3. Fees and costs

Fees for this engagement are set in a separate writing you and the Firm sign —
a flat monthly fee, an hourly rate, a contingency, or a combination — and that
writing controls the fee. __The Firm will not begin work before that writing is
signed__, so you always know the basis on which you are being charged before
anything is billed. Where the fee is contingent on a recovery, the rate is not
set by law and is negotiable, and it is written out in its own signed fee
agreement.

Fees do not include the administrator's filing fees, the arbitrator's
compensation, expert fees, court reporter and transcript costs, or other
third-party costs. Those are passed through at cost. The arbitrator's
compensation in a matter of this size is the largest of them, and the Firm will
give you an estimate before the arbitration is commenced rather than after.

Every invoice carries its own payment instructions. Read them there rather than
reusing instructions from an earlier invoice, and verify by telephone — at a
number you already know to be genuine — any emailed change to them, including
a message that appears to come from the Firm.

## 4. Who answers for the Matter on each side

Every matter the Firm opens names one person on each side who answers for it —
one lawyer here, one person on the client's side. We call each of them the
directly responsible individual, or DRI. Other people work on the matter; these
two answer for it, so you always know whom to ask where things stand. Neither
name changes except in writing.

* __The Firm's directly responsible individual is {{person__lawyer_dri}}__,
  who is principally responsible for this engagement — for the work, for the
  schedule, and for telling you candidly where the Matter stands.

* __The client's directly responsible individual is {{person__client}}__, the
  person the Firm takes instructions from and sends advice to.

## 5. Conflicts

The Firm treats a conflict for any one of its lawyers as a conflict for the
whole firm. Before taking on a new matter we check it against our current and
former matters; if that check turns up a conflict we cannot properly take on,
we tell you promptly, decline rather than wall it off internally, and return
any materials you shared with us.

You do not waive, and the Firm does not request, advance consent for the Firm
to appear adverse to you in any litigation, arbitration, or other contested
proceeding.

## 6. What we each do

You agree to provide accurate and complete information, to respond to
reasonable requests, and to make the decisions the representation needs — which
in an arbitration includes the choice of arbitrator and any decision to settle.
Those two are yours, not the Firm's. The Firm's advice depends on the
information available to it when the advice is given.

__The Firm has made no promise, assurance, or guarantee about the outcome__ of
the arbitration, of any negotiation, or of any settlement.

## 7. A disagreement between you and the Firm

This letter is governed by the law of {{custom_single_choice__governing_law}}.
If a dispute arises out of or relates to this engagement or this letter, you and
the Firm agree to resolve it by final and binding arbitration before a single
arbitrator administered by JAMS under its Comprehensive Arbitration Rules and
Procedures, conducted confidentially and decided under the law of
{{custom_single_choice__governing_law}}. The arbitrator applies the same law and
may award the same remedies a court could; this paragraph selects the forum for
a dispute and does __not__ limit, cap, or waive the Firm's responsibility for
its own work.

__Your fee-arbitration rights are preserved.__ Nothing above waives or
overrides any non-waivable statutory right you have to arbitration of a fee
dispute, including the fee-dispute program of the State Bar of Nevada.

Because this is an agreement about how future disputes are handled, you have
the right to consult independent counsel of your own choosing before you agree
to it.

## 8. Ending the engagement

You may end this engagement at any time by telling us. The Firm may withdraw as
the applicable professional rules permit or require — including for nonpayment,
a conflict, or a failure to cooperate — and subject to the rules governing
withdrawal from a pending proceeding. Fees and authorized expenses incurred
before that point remain due.

## 9. Signatures

Agreed and accepted:

{{person__client}}

By: ______________________________  Date: ____________

Neon Law, by {{person__lawyer_dri}}

By: ______________________________  Date: ____________
