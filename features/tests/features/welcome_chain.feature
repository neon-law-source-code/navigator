Feature: Navigator MCP sends a welcome email from a free-form A2A message

  Gemini Enterprise hands Navigator MCP free-form text with no skill named and
  no person_id — "send a welcome email to <addr>". The send tool only
  accepts a person_id (never a raw address, by design), so a single
  tool call can never satisfy the request: that is exactly why the old
  single-shot router resolved the address with show_person and then
  stopped. Navigator MCP's A2A handler now runs an agentic loop — it looks the
  person up to resolve the id, then sends, then finishes.

  The send is a real, client-facing action, so Navigator MCP does not run it
  unattended: the loop resolves the person inline (a read), then PAUSES
  before the side-effecting send and returns an "Authorize this action?"
  prompt. Reads run; writes wait. Only a firm-side principal (lawyer or
  admin) may authorize — the Model-Rule-5.3 supervision line — and they
  confirm in the same task with a "yes" before anything is sent.

  The router in this suite is a scripted stand-in for Gemini:
  deterministic, so CI never depends on a live model. That the *real*
  model chooses this chain is verified out-of-band against Vertex, not
  here. Everything below the router is the real thing — the real
  show_person and send_welcome_email tools, a real database, and the
  real welcome email rendered through the CapturingEmail backend.

  Background:
    Given a CapturingEmail-backed Neon Law Navigator app whose Navigator MCP router runs the lookup-then-send chain
    And a lawyer persons row for "Firm Lawyer" with email "lawyer@neonlaw.com"

  Scenario: A free-form welcome request pauses for authorization, then sends on yes
    Given a persons row for "Nick" with email "nick@neonlaw.com"
    When Navigator MCP receives the A2A message "send a welcome email to nick@neonlaw.com"
    Then Navigator MCP pauses for authorization to send the welcome email to "Nick"
    And no email has been captured yet
    When the firm authorizes the pending action with "yes"
    Then the A2A task completes with the welcome send as its artifact
    And exactly 1 captured email exists
    And the captured email is addressed to "nick@neonlaw.com"
    And the captured email subject is "Welcome to Neon Law"
