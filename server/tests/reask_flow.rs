#![allow(clippy::doc_markdown)]
//! Browser-driven end-to-end test of the re-ask loop (#252): a rejected
//! `lawyer_review` no longer dead-ends — lawyers flag the wrong answers, the
//! matter parks at `reask__client`, the flagged answers are re-collected,
//! and it loops back to review.
//!
//! Same harness contract as `browser_e2e`: probes for chromedriver + the
//! web server ([`new_client_or_skip`]) and skips cleanly when they aren't
//! up, so a plain `cargo test` stays green; runs for real under
//! `navigator dev e2e`. When `NAV_REASK_SHOTS=1`, saves screenshots of the
//! request-changes panel and the re-collection page under
//! `/tmp/navigator-screenshots/` for the PR walkthrough.

use std::time::Duration;

use features::webdriver::{base_url, login_as_lawyer, new_client_or_skip, wait_for_text};

const SET_INPUT: &str = "\
    const target = document.querySelector(arguments[0]); \
    target.value = arguments[1]; \
    target.dispatchEvent(new Event('input', {bubbles: true})); \
    target.dispatchEvent(new Event('change', {bubbles: true})); \
    return target.value;";

async fn set_input(c: &fantoccini::Client, selector: &str, value: &str) {
    c.execute(
        SET_INPUT,
        vec![
            serde_json::Value::String(selector.into()),
            serde_json::Value::String(value.into()),
        ],
    )
    .await
    .unwrap();
}

async fn submit(c: &fantoccini::Client, selector: &str) {
    c.execute(
        &format!("document.querySelector('{selector}').submit(); return true;"),
        vec![],
    )
    .await
    .unwrap();
}

/// Save a screenshot when `NAV_REASK_SHOTS=1`; a no-op otherwise so the
/// test's assertions run identically with or without capture.
async fn maybe_shot(c: &fantoccini::Client, name: &str) {
    if std::env::var("NAV_REASK_SHOTS").as_deref() != Ok("1") {
        return;
    }
    let dir = std::path::Path::new("/tmp/navigator-screenshots");
    std::fs::create_dir_all(dir).unwrap();
    let png = c.screenshot().await.unwrap();
    std::fs::write(dir.join(name), png).unwrap();
}

#[tokio::test]
async fn lawyer_sends_a_retainer_back_for_changes_and_it_loops_to_review() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&c).await;

    // --- Create the retainer notation + walk intake to lawyer_review ----
    let client_email = format!("reask-{}@example.com", std::process::id());
    c.goto(&format!("{}/app/lawyer/retainers/new", base_url()))
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(fantoccini::Locator::Css("input[name='client_email']"))
        .await
        .unwrap();
    set_input(&c, "input[name='client_email']", &client_email).await;
    set_input(
        &c,
        "select[name='retainer_template_code']",
        "onboarding__letter",
    )
    .await;
    submit(&c, "form.admin-form").await;

    // Walk the two retainer questions (person__client, project__engagement).
    for (i, value) in ["Libra", "Estate Plan — Libra"].iter().enumerate() {
        wait_for_text(&c, &format!("step {} of 2", i + 1), Duration::from_secs(15)).await;
        set_input(&c, "input[name=\"value\"], textarea[name=\"value\"]", value).await;
        submit(&c, "form.admin-form").await;
    }

    // --- Review page: the request-changes panel is offered -------------
    wait_for_text(&c, "Awaiting attorney review", Duration::from_secs(20)).await;
    let src = c.source().await.unwrap();
    assert!(
        src.contains("Request changes instead"),
        "review page must offer the request-changes panel",
    );
    // Open the <details> panel so the capture shows the flagging UI.
    c.execute(
        "const d = document.querySelector('details'); if (d) d.open = true; return true;",
        vec![],
    )
    .await
    .unwrap();
    maybe_shot(&c, "reask-1-request-changes.png").await;

    // Flag one answer + a note, then send it back for changes.
    c.execute(
        "document.querySelector('input[name=\"q:person__client\"]').checked = true; \
         const n = document.querySelector('#reask-note'); \
         if (n) n.value = 'Please confirm the spelling of the client\\'s legal name.'; \
         document.querySelector('form[action$=\"/request-changes\"]').submit(); return true;",
        vec![],
    )
    .await
    .unwrap();

    // --- Re-ask page: re-collect only the flagged answer ---------------
    wait_for_text(&c, "Re-collect flagged answers", Duration::from_secs(20)).await;
    let src = c.source().await.unwrap();
    assert!(
        src.contains("name=\"a:person__client\""),
        "re-ask page must re-collect the flagged question",
    );
    assert!(
        !src.contains("name=\"a:project__engagement\""),
        "re-ask page must re-collect ONLY the flagged question",
    );
    maybe_shot(&c, "reask-2-re-collect.png").await;

    // Capture the re-ask URL so we can return to a fresh form after the
    // guard rejects an incomplete resubmit.
    let reask_url = c.current_url().await.unwrap().to_string();

    // --- Guard: resubmitting with the flagged answer blank is refused --
    // `form.submit()` bypasses the input's `required` attribute, so this
    // exercises the server-side check: the matter must not return to review
    // with the wrong value still on record.
    submit(&c, "form[action$=\"/reask\"]").await;
    wait_for_text(
        &c,
        "re-collect every flagged answer",
        Duration::from_secs(10),
    )
    .await;

    // --- Correct the flagged answer and resubmit for review ------------
    c.goto(&reask_url).await.unwrap();
    wait_for_text(&c, "Re-collect flagged answers", Duration::from_secs(10)).await;
    set_input(&c, "input[name=\"a:person__client\"]", "Libra Jones").await;
    submit(&c, "form[action$=\"/reask\"]").await;

    // --- Loops back to review, never dead-ends -------------------------
    wait_for_text(&c, "Awaiting attorney review", Duration::from_secs(20)).await;

    c.close().await.unwrap();
}
