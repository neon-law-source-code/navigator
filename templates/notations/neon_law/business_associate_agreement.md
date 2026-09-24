---
kind: agreement
title: Business Associate Agreement
code: business__business_associate_agreement
jurisdiction: US
respondent_type: person_and_entity
confidential: false
origin_url: https://github.com/General-Legal/legal-templates/tree/main/templates/business-associate-agreement
prompts:
  company: "What is the company’s full legal name and entity type?"
  other_party: "What is the other party’s full legal name and entity type?"
  effective_date: "What date does this agreement take effect?"
  business_customer_role: "Is Customer a covered entity or an upstream business associate?"
  business_services: "Identify the service agreement and the specific HIPAA-covered services."
  business_security: "Describe actual administrative, technical, and physical safeguards."
  business_incident_deadline: "Specify the contractual outer reporting deadline, consistent with HIPAA."
  incident_contacts: "Who are the primary and alternate contacts? Include name, title, email, and postal address."
  business_rights: "State procedures and deadlines for access, amendment, and accounting requests."
  business_cure_period: "State the cure period for a curable material violation."
audiences:
  company: lawyer
  other_party: lawyer
  effective_date: lawyer
  business_customer_role: lawyer
  business_services: lawyer
  business_security: lawyer
  business_incident_deadline: lawyer
  incident_contacts: lawyer
  business_rights: lawyer
  business_cure_period: lawyer
questionnaire:
  BEGIN:
    _: entity__company
  entity__company:
    _: entity__other_party
  entity__other_party:
    _: custom_datetime__effective_date
  custom_datetime__effective_date:
    _: custom_text__business_customer_role
  custom_text__business_customer_role:
    _: custom_text__business_services
  custom_text__business_services:
    _: custom_text__business_security
  custom_text__business_security:
    _: custom_text__business_incident_deadline
  custom_text__business_incident_deadline:
    _: people__incident_contacts
  people__incident_contacts:
    _: custom_text__business_rights
  custom_text__business_rights:
    _: custom_text__business_cure_period
  custom_text__business_cure_period:
    _: END
  END: {}
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    approved: END
    changes_requested: reask__draft
  reask__draft:
    resubmitted: lawyer_review
  END: {}
---

<!--
Drafting note: Confirm covered-entity or subcontractor roles, permitted services, actual safeguards, reporting
periods, designated record sets, and retention. Check HIPAA and any separately applicable health-data laws
before approval. Adapted from General Legal’s CC0 template at revision
6d6805425eabd41bed86fc1e2ec51612760f716c. This is a review draft, not an executed instrument.
-->

# Business Associate Agreement

{{entity__company.name}} (Business Associate) and {{entity__other_party.name}} (Customer) enter this addendum on
{{custom_datetime__effective_date}}. Customer acts as {{custom_text__business_customer_role}}. The covered services and
underlying agreement are {{custom_text__business_services}}.

## I. Scope

Protected health information (PHI) means PHI received, created, maintained, or transmitted for Customer under the
covered services. HIPAA and its implementing Privacy, Security, Breach Notification, and Enforcement Rules supply the
meanings of their defined terms. This addendum controls conflicting service terms concerning PHI. It does not authorize
PHI in other services.

## II. Permitted use

Business Associate may use and disclose PHI only to perform the covered services as instructed by Customer, as required
by law, or as this addendum expressly permits. Uses, disclosures, and requests must meet applicable minimum-necessary
requirements. Business Associate may not sell PHI, use it for unrelated advertising, or train general-purpose AI models
on it. No de-identification or data aggregation is authorized without a separate written agreement consistent with
HIPAA.

Business Associate may use PHI for proper management and administration or legal responsibilities. Disclosure for those
purposes requires either a legal obligation or reasonable written assurances of confidentiality, limited purpose, and
notice of breaches. It may not otherwise use or disclose PHI in a way that would violate the Privacy Rule if done by
Customer.

## III. Safeguards and subcontractors

Business Associate will implement appropriate administrative, physical, and technical safeguards and comply with the
Security Rule for electronic PHI. Agreed security measures are: {{custom_text__business_security}}. Personnel may access
PHI only as needed for authorized work and must be bound by confidentiality duties. Subcontractors that create, receive,
maintain, or transmit PHI must accept the same applicable restrictions, conditions, and security requirements in
writing. Business Associate remains responsible for its obligations.

## IV. Incidents

Business Associate must report unauthorized uses or disclosures, security incidents, and breaches of unsecured PHI to
Customer without unreasonable delay, within {{custom_text__business_incident_deadline}}, and sooner if law requires. It
must provide known facts, affected individuals and information where available, mitigation, and continuing updates.
Initial notice must not wait for a completed investigation. Notice goes to {{#for c in people__incident_contacts}}
{{c.name}} ({{c.title}}): {{c.email}}; {{c.street}}, {{c.city}}, {{c.state}} {{c.zip}}, {{c.country}}.
{{/for}} Business Associate will mitigate harmful effects and cooperate with Customer’s legally required
notifications. An unsuccessful incident reporting arrangement requires a separate written description and must remain
consistent with law.

## V. Individual rights and oversight

Business Associate will make PHI available to Customer or its designated individual to support required access; make
directed amendments; and document and supply information for accountings of disclosures. The response procedure and
deadlines are {{custom_text__business_rights}}, which must allow Customer to meet legal deadlines. Direct requests from
individuals must be promptly forwarded to Customer unless Customer instructs otherwise.

To the extent Business Associate performs Customer’s Privacy Rule duties, it must comply with the rules applicable to
those duties. It will make relevant internal practices, books, and records available to the Secretary of HHS for
compliance review. This agreement does not restrict government oversight.

## VI. Customer duties

Customer will provide necessary notices, permissions, restrictions, and changes affecting lawful use of PHI. Customer
must not request a use or disclosure that would violate HIPAA. Each party remains responsible for its own legal duties.
Customer’s instructions do not excuse Business Associate’s independent obligations.

## VII. Term and termination

This addendum continues while Business Associate holds PHI. Customer may terminate the covered services for Business
Associate’s material violation if uncured within {{custom_text__business_cure_period}}, or immediately if cure is not
feasible. On termination, Business Associate must return or destroy all PHI, including subcontractor-held PHI, where
feasible. If infeasible, it must explain why, continue protections, and limit further use to the purpose making return
or destruction infeasible. Those duties survive while PHI remains.

## VIII. Administration

The parties will amend this addendum as necessary to comply with applicable HIPAA requirements. No provision creates
third-party contractual rights. Governing law, notices, and other service terms remain in the underlying agreement,
subject to this addendum and mandatory law. Liability terms cannot excuse either party’s regulatory duties. Counterparts
and electronic signatures are effective.

## Signatures

Business Associate: ____________________  Name and title: ____________________  Date: ____________________

Customer: ____________________  Name and title: ____________________  Date: ____________________
