---
kind: offboarding
title: Closing Letter
respondent_type: person_and_entity
code: offboarding__letter
jurisdiction: NV
confidential: true
prompts:
  client_name: Who is the Client's directly responsible individual, the one person the Firm writes to?
  project: What is the project being closed?
questionnaire:
  BEGIN:
    _: entity
  entity:
    _: person__client
  person__client:
    _: project
  project:
    _: END
  END: {}
workflow:
  BEGIN:
    close_requested: lawyer_review
  lawyer_review:
    approved: generate_pdf__closing_letter
    rejected: END
  generate_pdf__closing_letter:
    pdf_persisted: firm_signature__closing_letter
  firm_signature__closing_letter:
    signed: END
  END: {}
---

{{person__client.name}}

Re: Closing Letter — {{project.name}}

Dear {{person__client.name}}:

This Closing Letter confirms that Shook Law PLLC ("Neon Law") has completed its work for {{entity.name}} (the "Client")
on the matter referred to as {{project.code}}.

## I. Representation concluded

The Firm's representation of the Client on this matter is now concluded. The Firm will take no further action on this
matter. Should a new need arise, the Client is welcome to open a new matter with the Firm at any time.

## II. Work completed

The Client's project files are available to download from the Neon Law portal for three days after this letter is
signed.

## III. Fees at closing

Please find the attached PDF of the project fee schedule. If you are owed a balance, we will send a check to the address
on file.

## IV. Your file

We will make reasonable commercial efforts to store your data for ten years.

## V. Closing

It has been our privilege to do this work alongside you. If you wish to engage us in the future, it will require a new
retainer for a new project.
