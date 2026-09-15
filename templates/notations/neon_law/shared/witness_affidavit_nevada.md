---
kind: filing
title: Affidavit of Percipient Witness (Nevada)
jurisdiction: NV
respondent_type: person
code: witness_affidavit__nevada
confidential: false
questionnaire:
  BEGIN:
    _: person__client
  person__client:
    _: custom_datetime__offer_date
  custom_datetime__offer_date:
    _: custom_datetime__completion_date
  custom_datetime__completion_date:
    _: custom_datetime__discovery_date
  custom_datetime__discovery_date:
    _: END
  END: {}
custom_questions:
  offer_date:
    prompt: On what date was the doughnut offered at the hedge?
  completion_date:
    prompt: On what date was the remainder consumed?
  discovery_date:
    prompt: On what date did the client first mention the soul-conveyance term?
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
  END: {}
---

# AFFIDAVIT OF PERCIPIENT WITNESS

__FIXTURE DOCUMENT.__ This is a sample rendered from a notation template in the
Navigator sample project. _Cruller v. Prine_ is a simulated matter, no person
described here exists, and nothing in it is legal advice or sworn testimony.

__Matter:__ Cruller v. Prine — Count II

__Affiant:__ Odile Cruller

__Concerning:__ {{person__client}}

## 1. Basis of knowledge

I am over the age of eight and competent to testify to the matters stated
below. Each is within my personal knowledge. I keep a dated notebook and have
done so continuously throughout the period described.

## 2. The offer

On {{custom_datetime__offer_date}} I was in the yard. I saw the neighbor speak
to my father across the hedge and hand him a doughnut. I heard the neighbor
describe the doughnut. He used the word "neat." He did not use the word "soul,"
and he did not say that anything was written inside the doughnut.

I wrote the exchange down that evening. The entry is dated and has not been
altered.

## 3. The interval

Between {{custom_datetime__offer_date}} and {{custom_datetime__completion_date}}
the remainder of the doughnut was in our refrigerator. During that period my
father did not mention a soul, a contract, or the neighbor's aspect. My
notebook records no such conversation, and I would have recorded one.

## 4. The completion

On {{custom_datetime__completion_date}} my father ate the remainder. He said he
was hungry. He said nothing about an agreement, and nothing in his manner
suggested he believed he was performing one.

## 5. Discovery

On {{custom_datetime__discovery_date}} my father stated for the first time that
the doughnut had carried a term concerning his soul. He appeared to be learning
it rather than recalling it.

Odile Cruller

Subscribed and sworn before me: ______________________________
