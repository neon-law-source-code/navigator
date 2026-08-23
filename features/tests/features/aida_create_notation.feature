Feature: AIDA walks a notation to END under a lawyer's authorization

  An A2A client names the `create_notation` and `answer_notation`
  skills directly and drives the retainer questionnaire end to end.
  The server owns the questionnaire state machine; the client just
  relays prompts to the user and submits the answers.

  Creating or answering a Notation is a supervised act, because the
  artifact binds the client, so neither skill runs on the lawyer tier
  alone. Every call PAUSES in `input-required` and a firm principal
  authorizes it before the write happens: the agent proposes, a
  licensed human approves.

  That pause is why the walk lives here rather than on `/mcp`. MCP has
  no `input-required` state to stop in, so `/mcp` withholds both
  skills from its catalog and refuses one named anyway. The last
  scenario pins that refusal against the same running app, so the two
  transports' answers to the same act are proved side by side.

  Background:
    Given a fresh Neon Law Navigator app with the canonical templates seeded
    And a lawyer persons row for "Firm Lawyer" with email "lawyer@neonlaw.com"
    And a seeded person "Libra" with email "libra@example.com"
    And an open matter whose client is "libra@example.com"

  Scenario: A full retainer walk over A2A advances the questionnaire to END
    When the LLM names the create_notation skill for "onboarding__retainer" on that matter
    Then AIDA pauses for authorization to "Create Notation"
    When the firm authorizes the pending action
    Then the task completes with status "needs_answer"
    And the next question is "person__client"

    When the LLM names the answer_notation skill with code "person__client" value "Libra"
    Then AIDA pauses for authorization to "Answer Notation"
    When the firm authorizes the pending action
    Then the task completes with status "needs_answer"
    And the next question is "project__engagement"

    When the LLM names the answer_notation skill with code "project__engagement" value "Apollo"
    And the firm authorizes the pending action
    Then the task completes with status "needs_answer"
    And the next question is "custom_single_choice__governing_law"

    When the LLM names the answer_notation skill with code "custom_single_choice__governing_law" value "nevada"
    And the firm authorizes the pending action
    Then the task completes with status "complete"
    And the notation has reached the questionnaire END state

  Scenario: Answering with the wrong question code is rejected as invalid arguments
    When the LLM names the create_notation skill for "onboarding__retainer" on that matter
    And the firm authorizes the pending action
    Then the task completes with status "needs_answer"

    When the LLM names the answer_notation skill with code "custom_text__settlement_terms" value "Apollo"
    And the firm authorizes the pending action
    Then the task fails mentioning "person__client"

  Scenario: The same act named on /mcp is refused instead of run
    When the LLM calls aida_create_notation for "onboarding__retainer" on that matter over /mcp
    Then the MCP result refuses the act and routes the caller to the Navigator app
    And no notation exists on that matter
