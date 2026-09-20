---
kind: github
title: Create a GitHub Pull Request
questionnaire:
  BEGIN:
    _: custom_single_choice__change_surface
  custom_single_choice__change_surface:
    _: custom_yes_no__engineering_council
  custom_yes_no__engineering_council:
    _: custom_text__closes_issue
  custom_text__closes_issue:
    _: custom_text__change_summary
  custom_text__change_summary:
    _: custom_text__covering_tests
  custom_text__covering_tests:
    _: custom_text__gates_run
  custom_text__gates_run:
    _: custom_text__walkthrough
  custom_text__walkthrough:
    _: END
  END: {}
prompts:
  change_surface: What does this change touch?
  engineering_council: >-
    Did the Engineering Council review this? Say yes when the council was convened for
    the design and this pull request carries its consensus; no when the change was
    settled without one.
  closes_issue: >-
    Which issue does this close? Give the issue number, or say what grounded the work
    if it started somewhere else.
  change_summary: >-
    What changed, and why is this the smallest change that satisfies the evidence?
  covering_tests: >-
    Which test proves it? Name the test that lands with this change and what it would
    catch if the implementation regressed.
  gates_run: >-
    Which gates did you run, and what did they say? Report the ones that failed or
    that you skipped, not only the green ones.
  walkthrough: >-
    Where is the live walkthrough? Public and portal UI changes carry a captured GIF
    or screenshot; say "not user-visible" when there is nothing to show.
choices:
  change_surface:
    web: Web feature
    api: API feature
    infrastructure: Infrastructure
    form: Government form
---

## What changed

{{custom_text__change_summary}}

Change surface: **{{custom_single_choice__change_surface}}**

Closes: {{custom_text__closes_issue}}

## Covering tests

{{custom_text__covering_tests}}

## Gates run

{{custom_text__gates_run}}

## Walkthrough

{{custom_text__walkthrough}}

## Engineering Council

Reviewed by the council: {{custom_yes_no__engineering_council}}
