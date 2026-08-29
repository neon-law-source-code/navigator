Feature: Bundled-template questionnaire composition

  The engagement letter is BDD-tested end-to-end via the walker; the other
  bundled templates lock down their questionnaire composition here. The
  `workflow_integrity` workspace test owns generic engine invariants (BEGIN
  present, END reachable, every transition target exists, every workflow
  prefix resolves to a `StepKind`); these scenarios pin the catalog so an
  accidental reshape of a remaining template surfaces as a named failing
  scenario.

  Each template also gets one rejection scenario: a hand-mutilated copy with
  `END:` excised must fail to parse with `MissingEnd`, so the parser's
  guardrails stay load-bearing.

  Scenario: Engagement letter questionnaire walks entity → office → client → lawyer → project → terms → END
    Given the bundled template "neon_law/shared/engagement_letter.md"
    Then the questionnaire transitions, in BEGIN-first order, are:
      | from                                    | to                                      |
      | BEGIN                                   | entity                                  |
      | entity                                  | address__principal_office               |
      | address__principal_office               | person__client                          |
      | person__client                          | person__lawyer_dri                      |
      | person__lawyer_dri                      | project__engagement                     |
      | project__engagement                     | custom_datetime__engagement_start_date  |
      | custom_datetime__engagement_start_date  | custom_text__engagement_scope           |
      | custom_text__engagement_scope           | custom_single_choice__governing_law     |
      | custom_single_choice__governing_law     | END                                     |

  Scenario: Engagement letter template with END stripped fails to parse
    Given the bundled template "neon_law/shared/engagement_letter.md" with the workflow END declaration removed
    Then parsing the workflow spec returns a MissingEnd error

  Scenario: Closing letter questionnaire walks client → project → summary → fees → file → next → END
    Given the bundled template "neon_law/shared/offboarding_letter.md"
    Then the questionnaire transitions, in BEGIN-first order, are:
      | from                               | to                                 |
      | BEGIN                              | person__client                     |
      | person__client                     | project__engagement                |
      | project__engagement                | custom_text__matter_summary        |
      | custom_text__matter_summary        | custom_single_choice__fee_status   |
      | custom_single_choice__fee_status   | custom_text__file_retention        |
      | custom_text__file_retention        | custom_text__next_obligation       |
      | custom_text__next_obligation       | END                                |

  Scenario: Closing letter template with END stripped fails to parse
    Given the bundled template "neon_law/shared/offboarding_letter.md" with the workflow END declaration removed
    Then parsing the workflow spec returns a MissingEnd error

  Scenario: Nevada LLC formation questionnaire walks client → company → agent → management → members → date → END
    Given the bundled template "forms/united_states/nevada/state/nv__llc_formation.md"
    Then the questionnaire transitions, in BEGIN-first order, are:
      | from                                         | to                                           |
      | BEGIN                                        | person__client                               |
      | person__client                               | entity__company                              |
      | entity__company                              | person__registered_agent                     |
      | person__registered_agent                     | custom_single_choice__management_structure   |
      | custom_single_choice__management_structure   | people__managing_members                     |
      | people__managing_members                     | custom_datetime__formation_date              |
      | custom_datetime__formation_date              | END                                          |

  Scenario: Nevada LLC formation template with END stripped fails to parse
    Given the bundled template "forms/united_states/nevada/state/nv__llc_formation.md" with the workflow END declaration removed
    Then parsing the workflow spec returns a MissingEnd error
