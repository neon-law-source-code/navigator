#![allow(clippy::doc_markdown)]
//! Browser-driven end-to-end test against a live KIND cluster.
//!
//! The whole workspace is Rust only, and the WebDriver protocol
//! gets us a real Chromium session against `localhost:8080`.
//!
//! ## Prerequisites
//!
//! 1. KIND cluster up and seeded:
//!    `cargo run --release -p cli -- dev deploy` (see `AGENTS.md`, Local KIND development).
//! 2. Lawyer has the `lawyer` role granted in the store (see `AGENTS.md`, Authentication and lawyer access).
//! 3. `chromedriver` (or `geckodriver`) running on
//!    `http://localhost:9515`:
//!
//!    ```sh
//!    chromedriver --port=9515
//!    ```
//!
//! ## Run
//!
//! These tests are not `#[ignore]`'d: each probes for the harness
//! ([`new_client_or_skip`]) and skips cleanly when chromedriver or the
//! web server isn't reachable, so a plain `cargo test` (and CI without
//! the harness) stays green. With the harness up they run automatically:
//!
//! ```sh
//! cargo test -p server --test browser_e2e -- --test-threads=1
//! ```
//!
//! `NAV_BASE_URL` overrides the target (default `http://localhost:8080`);
//! `WEBDRIVER_URL` overrides the chromedriver location.

use std::env;
use std::time::Duration;

use fantoccini::key::Key;
use fantoccini::Locator;
use features::webdriver::{
    base_url, click_and_reach, login_as_admin, login_as_client, login_as_lawyer,
    new_client_or_skip, require_harness, scroll_and_js_click, wait_for_text,
    wait_for_text_reloading,
};
use uuid::Uuid;

/// Extract the notation id from a `/app/lawyer/notations/:id/step` path.
fn notation_id_from_step_path(path: &str) -> Option<Uuid> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["lawyer", "notations", id, "step"] => Uuid::parse_str(id).ok(),
        _ => None,
    }
}

#[test]
fn notation_id_from_step_path_accepts_lawyer_step_path() {
    let id = Uuid::parse_str("018f42d4-bf4a-73fe-9f7f-8a6c51f0c6b0").unwrap();

    assert_eq!(
        notation_id_from_step_path("/app/lawyer/notations/018f42d4-bf4a-73fe-9f7f-8a6c51f0c6b0/step"),
        Some(id)
    );
}

#[test]
fn notation_id_from_step_path_rejects_non_lawyer_step_path() {
    assert_eq!(
        notation_id_from_step_path(
            "/app/admin/notations/018f42d4-bf4a-73fe-9f7f-8a6c51f0c6b0/step"
        ),
        None
    );
}

#[tokio::test]
async fn home_page_renders() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    c.goto(&format!("{}/", base_url())).await.unwrap();
    let title = c.title().await.unwrap();
    assert!(
        title.to_lowercase().contains("neon law"),
        "expected `neon law` in page title, got `{title}`",
    );
    c.close().await.unwrap();
}

#[tokio::test]
async fn design_page_renders_the_dioxus_gallery() {
    // The /design gallery is a shared Navigator tool behind the session
    // boundary (#732) and renders the Dioxus Components, styled by the Dioxus
    // Components theme. After sign-in a real browser must show the inline-SVG
    // icons (no icon webfont) and a themed card — the SSR content the
    // render_handler produces, readable on load.
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&c).await;
    c.goto(&format!("{}/design", base_url())).await.unwrap();
    // Inline SVG icons, rendered by the Dioxus Icon component.
    let _icon = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("svg.nav-icon"))
        .await
        .unwrap();
    // A themed card is present.
    let _card = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(".nav-card"))
        .await
        .unwrap();
    // The brand-token swatches render, proving the theme stylesheet loaded and
    // the chip rules exist (an SSR test only sees the class name).
    let _swatch = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(".design-swatch__chip"))
        .await
        .unwrap();
    // The URL-contract demo table renders, with a real `?sort=` header anchor.
    let _sort_link = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(".nav-table th a[href*=\"sort=\"]"))
        .await
        .unwrap();
    c.close().await.unwrap();
}

#[tokio::test]
async fn lawyer_logs_in_and_reaches_the_signed_in_chrome() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&c).await;

    // Sanity: the signed-in chrome is present. The nav does not name the
    // viewer, so this looks for the sign-out link rather than a per-role
    // desk.
    let _ = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("a[href^='/auth/logout']"))
        .await
        .unwrap();

    // A signed-in 403, 404, or 500 renders that same sign-out link, so the
    // signal only means "signed-in chrome" once the error body is ruled out.
    assert!(
        c.find_all(Locator::Css(".error-page"))
            .await
            .unwrap()
            .is_empty(),
        "landed on an error page after signing in as lawyer",
    );

    c.close().await.unwrap();
}

/// Fresh `dev` boot smoke: unlike the upload scenario below this test
/// creates no data. It proves the Rauthy Lawyer harness can work
/// the sample-matter fixture that startup seeded.
#[tokio::test]
async fn stock_local_personas_reach_the_litigation_matter_through_their_own_lenses() {
    // This is the presenter dry run: the Rauthy fixture, development seed,
    // and browser-visible authorization all agree without a manual SQL grant.
    let Some(lawyer_browser) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&lawyer_browser).await;
    // The stock `lawyer` login is a paralegal participant on every seeded
    // litigation matter, so it reaches the project through the *lawyer* lens at
    // `/app/projects` — the workbench backed by `visible_projects_as_lawyer`. The
    // reload-aware wait rides out a workbench that is still settling immediately
    // after the deploy's rollout.
    wait_for_text_reloading(
        &lawyer_browser,
        &format!("{}/app/projects", base_url()),
        "Cruller v. Prine",
        Duration::from_secs(30),
    )
    .await;
    lawyer_browser.close().await.unwrap();

    let Some(client_browser) = new_client_or_skip().await else {
        return;
    };
    // Virgo is the seeded client of record, so the same matter is reached
    // through the *client* lens at `/app/projects`, which renders
    // `visible_projects_as_client` for a client-tier caller.
    login_as_client(&client_browser).await;
    wait_for_text_reloading(
        &client_browser,
        &format!("{}/app/projects", base_url()),
        "Cruller v. Prine",
        Duration::from_secs(30),
    )
    .await;
    client_browser.close().await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn lawyer_walks_the_full_retainer_questionnaire_end_to_end() {
    // Drives every leg of the stepwise retainer flow in a real
    // browser:
    //   1. POST /app/lawyer/retainers/new          → /step
    //   2. POST /step × 8 (one question each) → result page
    //
    // Preconditions (beyond the module's chromedriver + KIND
    // requirements): the `onboarding__letter` template must
    // be seeded via `navigator site seed templates/`, and
    // `store/seeds/Question.yaml` must be seeded so the record-backed
    // walker question codes (`entity`, `address`, `person`, `project`) are
    // looked up successfully.
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&c).await;

    // The eight answers we'll submit, in walker order (entity →
    // address__principal_office → person__client → person__lawyer_dri →
    // project__engagement → custom_datetime__engagement_start_date →
    // custom_text__engagement_scope → custom_single_choice__governing_law). The
    // values are unique enough that we can fish them back out of the rendered
    // result page.
    // `custom_datetime__engagement_start_date` renders a native
    // `<input type="datetime-local">` (its `Question.answer_type` is the
    // generic `datetime`, not `custom_datetime`) — the value must be a full
    // `YYYY-MM-DDTHH:MM` string, or the browser's own value setter silently
    // blanks an incomplete one before the form ever submits it.
    // `entity` and `address__principal_office` fall through
    // `question_fields`'s default arm to the same plain `<input name="value">`
    // as `person__client`/`project__engagement` (no dedicated `answer_type`
    // branch exists for either), so the generic
    // `input[name="value"], textarea[name="value"]` selector below drives them
    // too.
    let client_email = format!("walk-{}@example.com", std::process::id());
    let answers = [
        "Aurora Ridge Holdings LLC",
        "742 Meridian Ave, Reno, NV 89502",
        "Libra",
        "Firm Principal",
        "Estate Plan — Libra",
        "2026-09-01T00:00",
        "Draft and file the matter documents.",
        "Nevada",
    ];

    // --- Step 0: create the Notation -------------------------
    c.goto(&format!("{}/app/lawyer/retainers/new", base_url()))
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("input[name='client_email']"))
        .await
        .unwrap();
    // Set values via JS instead of send_keys (chromedriver
    // intermittently drops keystrokes on freshly-rendered forms).
    // `dispatchEvent('input')` keeps framework input listeners
    // happy and runs the browser's `required` validation against
    // the new value before we submit.
    let set_input_script = "\
        const target = document.querySelector(arguments[0]); \
        target.value = arguments[1]; \
        target.dispatchEvent(new Event('input', {bubbles: true})); \
        target.dispatchEvent(new Event('change', {bubbles: true})); \
        return target.value;";
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("input[name='client_email']".into()),
            serde_json::Value::String(client_email.clone()),
        ],
    )
    .await
    .unwrap();
    // `retainer_template_code` renders as a <select> dropdown (the
    // onboarding-template picker), not a text input — target the element
    // that actually exists, or `querySelector` returns null and the
    // `.value =` assignment throws.
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("select[name='retainer_template_code']".into()),
            serde_json::Value::String("onboarding__letter".into()),
        ],
    )
    .await
    .unwrap();
    // Submit the form directly — bypasses any quirks around
    // submit-button click-event delivery in fresh-loaded DOM.
    c.execute(
        "document.querySelector('form.admin-form').submit(); return true;",
        vec![],
    )
    .await
    .unwrap();

    // POST /retainers/new redirects to /app/lawyer/notations/:id/step.
    // Capture the id while we're here — we'll use it after the
    // walk to read the journal directly and confirm exactly
    // eight `notation_event` rows landed on the
    // questionnaire timeline.
    let started = std::time::Instant::now();
    let notation_id = loop {
        let url = c.current_url().await.unwrap();
        if let Some(id) = notation_id_from_step_path(url.path()) {
            break id;
        }
        assert!(
            started.elapsed() <= Duration::from_secs(10),
            "never landed on /app/lawyer/notations/:id/step; last URL was {url}",
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // --- Steps 1–8: walk the questionnaire -------------------
    for (i, value) in answers.iter().enumerate() {
        // Each step renders "step N of 8" — wait for the right
        // one to be sure we're looking at the form we expect.
        wait_for_text(&c, &format!("step {} of 8", i + 1), Duration::from_secs(10)).await;

        // Set the answer value via JS (chromedriver send_keys is
        // unreliable on freshly-rendered forms).
        c.execute(
            "\
            const target = document.querySelector(\
              'input[name=\"value\"], textarea[name=\"value\"]'); \
            target.value = arguments[0]; \
            target.dispatchEvent(new Event('input', {bubbles: true})); \
            target.dispatchEvent(new Event('change', {bubbles: true})); \
            return target.value;",
            vec![serde_json::Value::String((*value).to_string())],
        )
        .await
        .unwrap();
        c.execute(
            "document.querySelector('form.admin-form').submit(); return true;",
            vec![],
        )
        .await
        .unwrap();
    }

    // --- Result page: parked at the lawyer_review human gate --
    // The second submit completes intake and parks the notation at
    // `lawyer_review` — a true human gate. No PDF is rendered on the
    // client's completion request; the result page shows the parked
    // state, the substituted template body, and the attorney approve
    // action.
    wait_for_text(&c, "Awaiting attorney review", Duration::from_secs(20)).await;
    let src = c.source().await.unwrap();
    for value in &answers {
        assert!(
            src.contains(value),
            "rendered retainer is missing `{value}`",
        );
    }

    // --- Attorney writes the fee terms as a clause -----------
    // The engagement agreement leaves the fee to a custom clause, and the
    // send gate refuses an empty one (`ClausesRequired`). Add one before
    // approving; a
    // clause routes the notation back through review, so it has to land before
    // the approve rather than after it.
    c.goto(&format!(
        "{}/app/lawyer/notations/{notation_id}/clauses",
        base_url()
    ))
    .await
    .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("textarea[name='body']"))
        .await
        .unwrap();
    c.execute(
        "const t = document.querySelector('textarea[name=\"body\"]'); \
         t.value = arguments[0]; \
         t.dispatchEvent(new Event('input', {bubbles: true})); \
         document.querySelector('form.admin-form').submit(); return true;",
        vec![serde_json::Value::String(
            "The Firm's fee for this engagement is a flat fee stated to the Client in writing."
                .into(),
        )],
    )
    .await
    .unwrap();
    // The clause POST redirects to the clauses list; return to the review
    // screen, which now offers approve with a non-empty fee clause.
    wait_for_text_reloading(
        &c,
        &format!("{}/app/lawyer/notations/{notation_id}/review", base_url()),
        "Awaiting attorney review",
        Duration::from_secs(20),
    )
    .await;

    // --- Attorney approves: renders + parks the PDF ----------
    // Submitting the approve form fires `approved`; the worker renders +
    // persists the PDF on entering `generate_pdf__*` and the page offers
    // the deliberate send action.
    c.execute(
        "document.querySelector('form[action$=\"/approve-send\"]').submit(); return true;",
        vec![],
    )
    .await
    .unwrap();
    wait_for_text(&c, "Send for signature", Duration::from_secs(20)).await;

    // --- Attorney sends: the binding envelope goes out -------
    c.execute(
        "document.querySelector('form[action$=\"/send\"]').submit(); return true;",
        vec![],
    )
    .await
    .unwrap();
    wait_for_text(&c, "sent_for_signature__pending", Duration::from_secs(20)).await;

    c.close().await.unwrap();

    // --- Journal: read `notation_event` from SurrealDB --------
    // Talks to the in-cluster SurrealDB through `navigator dev up`'s
    // port-forward; `NAVIGATOR_SURREAL_ENDPOINT` etc. are exported by
    // `.devx/env`. We read through the same `store::notation_events` seam
    // the worker writes through, so the assertion exercises the wire
    // shape end-to-end without shelling out to a client.
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let mut events = store::notation_events::for_notation(&surreal, notation_id)
        .await
        .expect("read notation_event from surreal");
    events.retain(|e| e.machine_kind == store::notation_events::MACHINE_QUESTIONNAIRE);

    // Nine rows: BEGIN → entity → address__principal_office → person__client →
    // person__lawyer_dri → project__engagement →
    // custom_datetime__engagement_start_date → custom_text__engagement_scope →
    // custom_single_choice__governing_law → END. The walker signals the worker
    // once per question (eight times) and once more for the trailer-to-END in
    // the last POST.
    assert_eq!(
        events.len(),
        9,
        "expected 9 questionnaire transitions for notation {notation_id}, got {events:?}",
    );
    let states: Vec<(&str, &str, &str)> = events
        .iter()
        .map(|e| {
            (
                e.from_state.as_str(),
                e.to_state.as_str(),
                e.condition.as_str(),
            )
        })
        .collect();
    assert_eq!(
        states,
        vec![
            ("BEGIN", "entity", "_"),
            ("entity", "address__principal_office", "_"),
            ("address__principal_office", "person__client", "_"),
            ("person__client", "person__lawyer_dri", "_"),
            ("person__lawyer_dri", "project__engagement", "_"),
            (
                "project__engagement",
                "custom_datetime__engagement_start_date",
                "_"
            ),
            (
                "custom_datetime__engagement_start_date",
                "custom_text__engagement_scope",
                "_"
            ),
            (
                "custom_text__engagement_scope",
                "custom_single_choice__governing_law",
                "_"
            ),
            ("custom_single_choice__governing_law", "END", "_"),
        ],
        "questionnaire walked the wrong path",
    );
    // Payload assertions: the walker now threads the respondent's
    // answer through the signal so each of the eight answered
    // transitions carries `{"answer_value": "..."}`. The trailing
    // `custom_single_choice__governing_law → END` row has no answer and stays
    // NULL. Build the expected JSON via the same `answer_payload`
    // helper the worker uses so a future change to the JSON shape
    // can't desync the test from production.
    let expected_payloads: Vec<Option<String>> = answers
        .iter()
        .map(|v| Some(store::notation_events::answer_payload(v)))
        .chain(std::iter::once(None))
        .collect();
    let actual_payloads: Vec<Option<String>> = events.iter().map(|e| e.payload.clone()).collect();
    assert_eq!(
        actual_payloads, expected_payloads,
        "journal payload column does not match the answers the walker submitted",
    );
}

#[tokio::test]
async fn lawyer_user_can_hit_every_admin_route() {
    // Walks the same admin routes the in-process test
    // (`oidc_e2e::user_with_db_lawyer_role_can_hit_every_admin_route`)
    // covers, but through a real browser end-to-end. `/app/lawyer/people` is absent
    // since ENG-304 deleted the browser mirror — the one people surface is
    // `/app/admin/people`, which a lawyer is answered 403 at.
    let routes = [
        "/app/lawyer",
        "/app/admin/entities",
        "/app/admin/jurisdictions",
        "/app/admin/entity-types",
        "/app/admin/templates",
        "/app/admin/questions",
        "/app/projects",
    ];

    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&c).await;

    // Each route should render without a server-error status. WebDriver does
    // not expose the HTTP status directly, so we check for a non-error body:
    // a signed-in view carries the sign-out link, and no rendered view carries
    // the error-page body. Sign-out is the broader signal — several back-office
    // pages emit it without a matter link — so it survives a page that has not
    // picked up the matter nav.
    for route in routes {
        c.goto(&format!("{}{route}", base_url())).await.unwrap();
        let nav_links = c
            .find_all(Locator::Css("a[href^='/auth/logout']"))
            .await
            .unwrap();
        assert!(
            !nav_links.is_empty(),
            "expected the signed-in nav on {route}; got no /auth/logout links — was access denied?",
        );
        assert!(
            c.find_all(Locator::Css(".error-page"))
                .await
                .unwrap()
                .is_empty(),
            "expected {route} to render its view; got an error page",
        );
    }

    c.close().await.unwrap();
}

#[tokio::test]
async fn admin_adds_a_person_through_the_people_form() {
    // Drives the full browser create path end-to-end: the Dioxus "Add person"
    // form (#641 Phase 3) is a native `POST /app/admin/people` carrying the session
    // cookie plus the hidden `_csrf` field, and the handler answers a 303 back to
    // the list where the new row shows. This exercises the whole credential-keyed
    // CSRF path in a real browser — the thing rendering tests can't prove.
    //
    // Admin, not Lawyer: ENG-304 deleted the `/app/lawyer/people` mirror, so this is
    // the only browser form that creates a Person.
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_admin(&c).await;

    // Unique email per run so re-runs don't trip the uniqueness guard.
    let email = format!("e2e-person-{}@example.com", std::process::id());

    c.goto(&format!("{}/app/admin/people/new", base_url()))
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("input[name='name']"))
        .await
        .unwrap();

    // Set values via JS (chromedriver send_keys is flaky under load) then
    // fire input/change so any listeners see the value.
    let set_input_script = "\
        const target = document.querySelector(arguments[0]); \
        target.value = arguments[1]; \
        target.dispatchEvent(new Event('input', {bubbles: true})); \
        target.dispatchEvent(new Event('change', {bubbles: true})); \
        return target.value;";
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("input[name='name']".into()),
            serde_json::Value::String("E2E Person".into()),
        ],
    )
    .await
    .unwrap();
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("input[name='email']".into()),
            serde_json::Value::String(email.clone()),
        ],
    )
    .await
    .unwrap();

    // Fire the submit button's click via JS to submit the native form (a
    // WebDriver click can be intercepted when the button sits below the fold).
    scroll_and_js_click(&c, "form.admin-form button[type=\"submit\"]").await;

    // The 303 lands us on the list; the new row must be there — proof the
    // cookie + hidden `_csrf` write went through the native create handler.
    wait_for_text(&c, &email, Duration::from_secs(10)).await;
    let url = c.current_url().await.unwrap();
    assert_eq!(
        url.path(),
        "/app/admin/people",
        "a successful create should redirect to the people list, got {url}",
    );

    c.close().await.unwrap();
}

#[tokio::test]
async fn lawyer_creates_a_client_inline_on_the_project_form() {
    // Drives the inline "New client" form on /app/projects/new end to end in
    // a real browser. The form is a native <details> disclosure with its own
    // POST, answered post/redirect/get. What the browser has to prove: after
    // the create, the new client is the selected option in the matter form's
    // Client DRI picker. Rendering tests cannot prove the redirect round-trip
    // actually preselects it; this does.
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    login_as_lawyer(&c).await;

    let client_email = format!("inline-client-{}@example.com", std::process::id());

    c.goto(&format!("{}/app/projects/new", base_url()))
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("input[name='name']"))
        .await
        .unwrap();

    let set_input_script = "\
        const target = document.querySelector(arguments[0]); \
        target.value = arguments[1]; \
        target.dispatchEvent(new Event('input', {bubbles: true})); \
        target.dispatchEvent(new Event('change', {bubbles: true})); \
        return target.value;";

    // Open the "New client" disclosure and fill its native form.
    scroll_and_js_click(&c, "details.inline-create:last-of-type > summary").await;
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("input[name='client_name']"))
        .await
        .unwrap();
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("input[name='client_name']".into()),
            serde_json::Value::String("Inline Libra".into()),
        ],
    )
    .await
    .unwrap();
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("input[name='client_email']".into()),
            serde_json::Value::String(client_email.clone()),
        ],
    )
    .await
    .unwrap();
    c.execute(
        "document.querySelector('form[action=\"/app/projects/new/client\"]').submit(); \
         return true;",
        vec![],
    )
    .await
    .unwrap();

    // The redirect lands back on the form with `?client=<id>`, and the DRI
    // picker renders that client selected. Poll until it is chosen.
    let started = std::time::Instant::now();
    loop {
        let selected = c
            .execute(
                "const s = document.querySelector('#client_dri_person_id'); \
                 if (!s) return ''; \
                 const o = s.options[s.selectedIndex]; \
                 return o ? o.textContent : '';",
                vec![],
            )
            .await
            .unwrap();
        if selected
            .as_str()
            .is_some_and(|t| t.contains("Inline Libra"))
        {
            break;
        }
        assert!(
            started.elapsed() <= Duration::from_secs(10),
            "the new client never became the selected DRI; last was {selected:?}",
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // No HTMX, no Bootstrap modal, and no leftover dialog on the page.
    let modals = c
        .execute(
            "return document.querySelectorAll('.modal, [data-bs-toggle]').length;",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        modals.as_u64(),
        Some(0),
        "the inline create must not ship a Bootstrap modal any more",
    );

    c.close().await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn admin_edits_matter_participation_from_the_project_workbench() {
    // A real admin session uses the matter workbench to add and then edit a
    // participation row. The final database read proves the browser POSTs
    // reached the project-scoped ledger, rather than only changing the DOM.
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    // The store the running `web` reads.
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let lawyer = store::persons::find_by_email_ci(&surreal, "lawyer@neonlaw.com")
        .await
        .expect("look up lawyer person")
        .expect("the browser harness requires `navigator dev grant-lawyer`");
    let original_role = lawyer.role;
    let lawyer_id = lawyer.id;
    store::persons::set_role(&surreal, lawyer_id, store::persons::Role::Admin)
        .await
        .expect("promote the browser-harness lawyer for this admin-only flow");
    login_as_lawyer(&c).await;
    let unique = Uuid::now_v7();
    let candidate = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            format!("Participation E2E {unique}"),
            format!("participation-e2e-{unique}@example.com"),
            store::persons::Role::Lawyer,
        ),
    )
    .await
    .expect("seed participation candidate");
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            format!("Participation Client {unique}"),
            format!("participation-client-{unique}@example.com"),
            store::persons::Role::Client,
        ),
    )
    .await
    .expect("seed project client");
    // The edit step re-points the row at a second person to prove the write
    // re-derives participation from that person's tier — `clerk`, not `lawyer`.
    let replacement = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            format!("Participation Clerk {unique}"),
            format!("participation-clerk-{unique}@example.com"),
            store::persons::Role::Clerk,
        ),
    )
    .await
    .expect("seed participation replacement");
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("participation-e2e-{unique}"),
            name: format!("Participation E2E Matter {unique}"),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("seed project");
    store::projects::add_participation(&surreal, project.id, lawyer.id, "lawyer_dri")
        .await
        .expect("disclose logged-in lawyer to project");

    c.goto(&format!("{}/app/projects/{}", base_url(), project.code))
        .await
        .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        client.id,
        store::projects::DriSide::Client,
    )
    .await
    .expect("designate client DRI");
    wait_for_text(&c, "Matter people", Duration::from_secs(10)).await;
    // Drive the "Add person" link through the shared click-and-navigate
    // helper. A native WebDriver `.click()` here flaked in CI: the request
    // never reached `web` (empty pod logs, issue #512), meaning the element
    // was scrolled off-screen or briefly covered and the click landed without
    // triggering navigation. `click_and_reach` scrolls-then-JS-clicks to
    // bypass the interactability race and proves navigation actually happened
    // before we blame the form for a missing selector.
    click_and_reach(
        &c,
        &format!("a[href='/app/projects/{}/people/new']", project.code),
        &format!("/app/projects/{}/people/new", project.code),
        Duration::from_secs(10),
    )
    .await;
    // The page navigated to the form route; now prove it rendered the person
    // selector. A failure here is a genuine render fault rather than a lost
    // navigation, and the diagnostic reports the URL and source defensively so
    // a failed read can't mask the original selector error.
    if let Err(error) = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("select[name='person_id']"))
        .await
    {
        let url = c.current_url().await.map_or_else(
            |url_error| format!("<failed to read form page URL: {url_error}>"),
            |url| url.to_string(),
        );
        let source = c.source().await.unwrap_or_else(|source_error| {
            format!("<failed to read form page source: {source_error}>")
        });
        panic!(
            "project participation form did not render its person selector at {url}: {error}\n{source}"
        );
    }
    let set_input_script = "\
        const target = document.querySelector(arguments[0]); \
        target.value = arguments[1]; \
        target.dispatchEvent(new Event('input', {bubbles: true})); \
        target.dispatchEvent(new Event('change', {bubbles: true})); \
        return target.value;";
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("select[name='person_id']".into()),
            serde_json::Value::String(candidate.id.to_string()),
        ],
    )
    .await
    .unwrap();
    // No participation control to fill: the person picker is the whole form.
    c.execute(
        "document.querySelector('form.admin-form').submit(); return true;",
        vec![],
    )
    .await
    .unwrap();
    wait_for_text(&c, &candidate.name, Duration::from_secs(10)).await;

    let participation =
        store::projects::participation_for_person(&surreal, candidate.id, project.id)
            .await
            .expect("load browser-created participation")
            .expect("browser add inserted participation");
    assert_eq!(
        participation.participation, "lawyer",
        "the add derived its participation from the candidate's tier"
    );
    click_and_reach(
        &c,
        &format!(
            "a[href='/app/projects/{}/people/{}/edit']",
            project.code, participation.id
        ),
        &format!(
            "/app/projects/{}/people/{}/edit",
            project.code, participation.id
        ),
        Duration::from_secs(10),
    )
    .await;
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("select[name='person_id']"))
        .await
        .unwrap();
    c.execute(
        set_input_script,
        vec![
            serde_json::Value::String("select[name='person_id']".into()),
            serde_json::Value::String(replacement.id.to_string()),
        ],
    )
    .await
    .unwrap();
    c.execute(
        "document.querySelector('form.admin-form').submit(); return true;",
        vec![],
    )
    .await
    .unwrap();
    wait_for_text(&c, &replacement.name, Duration::from_secs(10)).await;
    let persisted = store::projects::participation_by_id(&surreal, participation.id)
        .await
        .expect("load edited participation")
        .expect("edited participation remains");
    assert_eq!(persisted.person_id, replacement.id);
    assert_eq!(
        persisted.participation, "clerk",
        "the edit re-derived participation from the incoming person's tier"
    );

    store::persons::set_role(&surreal, lawyer_id, original_role)
        .await
        .expect("restore the browser-harness lawyer role");
    c.close().await.unwrap();
}

/// Seed a project the browser-harness lawyer account can open through
/// `/app/projects/:code`, and return its id.
///
/// `can_access_as_lawyer` scopes a non-admin lawyer to projects
/// carrying a non-client participation row for them, so the fixture adds
/// one for `lawyer@neonlaw.com` — the account `login_as_lawyer` drives and
/// `navigator dev grant-lawyer` seeds.
async fn seed_lawyer_upload_project(surreal: &store::surreal::SurrealDb) -> String {
    let lawyer = store::persons::find_by_email_ci(surreal, "lawyer@neonlaw.com")
        .await
        .expect("look up the browser-harness lawyer person")
        .expect("lawyer@neonlaw.com must exist — `navigator dev grant-lawyer`");

    let entity_id = store::test_support::seed_entity(surreal).await;
    let unique = Uuid::now_v7();
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("batch-upload-{unique}"),
            name: format!("Batch Upload {unique}"),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .expect("seed the upload project");
    store::projects::add_participation(surreal, project.id, lawyer.id, "attorney")
        .await
        .expect("scope the harness lawyer person onto the project");

    project.code
}

/// Seed a project the browser-harness lawyer can open, returning its id and its
/// code. The matter page is keyed by id; the client-portal mount is keyed by
/// code, so the test needs both.
async fn seed_portal_project(surreal: &store::surreal::SurrealDb) -> (Uuid, String) {
    let lawyer = store::persons::find_by_email_ci(surreal, "lawyer@neonlaw.com")
        .await
        .expect("look up the browser-harness lawyer person")
        .expect("lawyer@neonlaw.com must exist — `navigator dev grant-lawyer`");

    let entity_id = store::test_support::seed_entity(surreal).await;
    let unique = Uuid::now_v7();
    let code = format!("portal-link-{unique}");
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: format!("Portal Link {unique}"),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .expect("seed the portal project");
    store::projects::add_participation(surreal, project.id, lawyer.id, "attorney")
        .await
        .expect("scope the harness lawyer person onto the project");

    (project.id, code)
}

/// The marker the harness entrypoint carries, and the only thing the portal
/// fixtures below assert on. Its `id` is what a `Locator::Css` selects; its
/// text is what proves the bytes came from the bucket rather than from an
/// error page that happens to render.
///
/// [`publish_portal_harness`] spells it out again rather than interpolating
/// this constant, because the body it publishes is a byte-string literal.
const PORTAL_HARNESS_MARKER: &str = "portal-harness-ok";

/// Publish a minimal bundle for `code` into the same applications bucket `web`
/// streams from, so a portal request renders a real entrypoint rather than the
/// non-disclosing 404 an unpublished Project serves.
///
/// Only `index.html` is published, deliberately: the fallback rule is that any
/// unmatched path below the mount resolves to the entrypoint, so publishing a
/// single object is enough to exercise the bare mount, the slashed root, and a
/// client-side deep link — and a second object would make it ambiguous which
/// of those a passing assertion actually proved.
async fn publish_portal_harness(code: &str) {
    let applications = cloud::applications_from_env()
        .await
        .expect("resolve the applications bucket from the sourced .devx/env");
    applications
        .put(
            &format!("{code}/portal/index.html"),
            b"<!doctype html><html><head><title>Portal harness</title></head>\
              <body><h1 id=\"portal-harness-ok\">Portal streams</h1></body></html>",
            "text/html",
        )
        .await
        .expect("publish the harness portal bundle");
}

/// Wait for the published entrypoint to be on screen, and return its heading
/// text — the one assertion every portal fixture below ends on.
async fn portal_bundle_heading(c: &fantoccini::Client) -> String {
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(&format!("#{PORTAL_HARNESS_MARKER}")))
        .await
        .expect("the published portal bundle streams")
        .text()
        .await
        .unwrap()
}

/// The matter page links to the Project's client portal, and clicking it streams
/// the published bundle.
///
/// This is the browser half of the coverage `portal/tests/project_portal_route.rs`
/// only reaches at the router level: it proves the link is rendered on the matter
/// page a firm viewer sees and that following it streams the bundle from the same
/// applications bucket `web` serves. The serve gate is `can_see_project`, the same
/// predicate that renders the matter page, so the link is live for this viewer.
#[tokio::test]
async fn the_project_page_links_to_the_client_portal_and_it_streams() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let (_project_id, code) = seed_portal_project(&surreal).await;

    publish_portal_harness(&code).await;

    login_as_lawyer(&c).await;
    c.goto(&format!("{}/app/projects/{code}", base_url()))
        .await
        .unwrap();

    let href = format!("/app/projects/{code}/portal/");
    let link = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(&format!("a[href='{href}']")))
        .await
        .expect("the matter page renders the Client portal link");
    assert_eq!(
        link.text().await.unwrap(),
        "Client portal",
        "the link carries the shared portal label"
    );

    // Following the link streams the published bundle from the applications
    // bucket — proof the link, the route, and the participation-gated serve all
    // line up end to end.
    //
    // `click_and_reach`, never a native `link.click()`. A WebDriver click
    // dispatches one pointer event at the element's in-view center and returns
    // on dispatch, so it can land on nothing and report success — issue #512.
    // This fixture was the one click-through in the suite that still used the
    // native call, and deploy run 32208649130 is what that costs: three
    // retries timed out here while the web pod's complete log named no gate
    // and Garage was never asked for the bundle, because the navigation was
    // never made. Reaching the path first is also what keeps a lost navigation
    // from being reported as a failure of the bundle below it.
    click_and_reach(
        &c,
        &format!("a[href='{href}']"),
        &href,
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(portal_bundle_heading(&c).await, "Portal streams");
}

/// The bare mount lands a browser on the published entrypoint.
///
/// A reader who types or bookmarks `/app/projects/{code}/portal` — no trailing
/// slash — must end up on the slashed root, because a Vite base joins every
/// asset URL directly onto it and the unslashed form would resolve them one
/// segment too high. `portal/tests/project_portal_route.rs` asserts the `301`
/// and its `Location`; this asserts the browser actually follows it and the
/// bundle renders, which is the part a status code cannot prove.
#[tokio::test]
async fn the_bare_portal_mount_lands_the_browser_on_the_published_entrypoint() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let (_project_id, code) = seed_portal_project(&surreal).await;
    publish_portal_harness(&code).await;

    login_as_lawyer(&c).await;
    c.goto(&format!("{}/app/projects/{code}/portal", base_url()))
        .await
        .unwrap();

    assert_eq!(portal_bundle_heading(&c).await, "Portal streams");
    assert_eq!(
        c.current_url().await.unwrap().path(),
        format!("/app/projects/{code}/portal/"),
        "the bare mount must leave the browser on the slashed root the bundle's \
         asset URLs are joined onto"
    );
}

/// A client-side route survives a browser refresh.
///
/// The bundle is a single-page application, so `/…/portal/dashboard/matters`
/// names a route inside it and no published object. Navigating straight there —
/// which is what a refresh, a bookmark, or a shared link does — must serve the
/// entrypoint rather than 404, or every deep link into a client's portal breaks
/// the moment they reload. Only `index.html` is published here, so reaching the
/// marker can only be the fallback resolving.
#[tokio::test]
async fn a_portal_deep_link_falls_back_to_the_published_entrypoint() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let (_project_id, code) = seed_portal_project(&surreal).await;
    publish_portal_harness(&code).await;

    login_as_lawyer(&c).await;
    c.goto(&format!(
        "{}/app/projects/{code}/portal/dashboard/matters",
        base_url()
    ))
    .await
    .unwrap();

    assert_eq!(portal_bundle_heading(&c).await, "Portal streams");
    assert_eq!(
        c.current_url().await.unwrap().path(),
        format!("/app/projects/{code}/portal/dashboard/matters"),
        "the fallback serves the entrypoint in place, without redirecting the \
         client-side route away"
    );
}

/// A signed-in client who is not on the matter never reaches its portal.
///
/// The serve gate is `store::access::can_see_project`, which reads the
/// participation ledger and carries no Owner/Admin bypass — and the bundle is
/// streamed through the handler rather than redirected to, so participation is
/// rechecked on every object. The fixture client is seeded onto `sample-litigation` and
/// onto no matter this test creates, which is exactly the shape that must be
/// refused: authenticated, same tier as a real portal reader, wrong matter.
///
/// The answer is the non-disclosing 404, never a 403 — a 403 would confirm to a
/// stranger that a Project with this code exists.
#[tokio::test]
async fn a_client_on_another_matter_never_reaches_this_portal() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let (_project_id, code) = seed_portal_project(&surreal).await;
    publish_portal_harness(&code).await;

    login_as_client(&c).await;
    c.goto(&format!("{}/app/projects/{code}/portal/", base_url()))
        .await
        .unwrap();

    // `project_portal::not_found` answers with a bare `Not Found` body rather
    // than the styled 404 page the router fallback renders. That is the shape
    // to assert: the mount refuses before any Navigator chrome is composed, so
    // the response carries nothing about the deployment, the viewer, or
    // whether a Project with this code exists.
    wait_for_text(&c, "Not Found", Duration::from_secs(10)).await;
    let source = c.source().await.unwrap();
    assert!(
        !source.contains(PORTAL_HARNESS_MARKER),
        "a nonparticipant must never receive the published bundle: {source}"
    );
    assert!(
        !source.contains("Forbidden"),
        "the refusal must be the non-disclosing 404, never a 403 confirming \
         that a Project with this code exists: {source}"
    );
}

#[tokio::test]
async fn lawyer_uploads_several_documents_at_once_from_the_project_page() {
    // The document picker is `multiple`, so one trip through the file
    // dialog can file a whole batch. This drives the real control: it
    // hands chromedriver three newline-separated paths (the WebDriver
    // idiom for a multi-select `input[type=file]`), submits the form,
    // and asserts all three land as separate documents on the page.
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the port-forwarded SurrealDB");
    let project_code = seed_lawyer_upload_project(&surreal).await;

    // Real files on disk — chromedriver uploads from the filesystem, so
    // these can't be synthesized in the page.
    let unique = Uuid::now_v7();
    let dir = std::env::temp_dir().join(format!("navigator-batch-upload-{unique}"));
    std::fs::create_dir_all(&dir).expect("create the upload scratch dir");
    let names = ["alpha.txt", "beta.txt", "gamma.txt"];
    let mut paths = Vec::new();
    for name in names {
        let path = dir.join(name);
        std::fs::write(&path, format!("contents of {name} for {unique}"))
            .expect("write an upload fixture file");
        paths.push(path.to_string_lossy().into_owned());
    }

    login_as_lawyer(&c).await;
    c.goto(&format!("{}/app/projects/{project_code}", base_url()))
        .await
        .unwrap();

    let picker = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css("section.project-documents input[type='file']"))
        .await
        .expect("the documents section renders its upload picker");

    // The picker must be multi-select, or the batch below is meaningless.
    assert_eq!(
        picker.attr("multiple").await.unwrap().as_deref(),
        Some("true"),
        "the project document picker should accept more than one file"
    );

    // WebDriver's contract for a multi-select file input: one path per
    // line, sent to the element in a single send_keys.
    picker
        .send_keys(&paths.join("\n"))
        .await
        .expect("hand the batch of paths to the file input");

    scroll_and_js_click(
        &c,
        "section.project-documents form.admin-form button[type=\"submit\"]",
    )
    .await;

    // The handler redirects back to the project page; every file in the
    // batch should now be its own row in the documents table.
    for name in names {
        wait_for_text(&c, name, Duration::from_secs(20)).await;
    }

    let source = c.source().await.unwrap();
    for name in names {
        assert!(
            source.contains(name),
            "expected {name} to appear as its own document row"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
    c.close().await.unwrap();
}

/// Send a chord of keys to the focused element as one real keyboard action:
/// press each in order, then release in reverse. Proves the Catalog step
/// page's arrow-key navigation and its guards against a real browser, not a
/// synthesized DOM event.
async fn press_chord(c: &fantoccini::Client, keys: &[char]) {
    use fantoccini::actions::{InputSource, KeyAction, KeyActions};
    let mut act = KeyActions::new("catalog-kbd".to_string());
    for &k in keys {
        act = act.then(KeyAction::Down { value: k });
    }
    for &k in keys.iter().rev() {
        act = act.then(KeyAction::Up { value: k });
    }
    c.perform_actions(act).await.unwrap();
}

/// Poll until the current URL ends with `suffix` (5s budget). A passing
/// assertion here is the proof that a keystroke actually navigated.
async fn wait_url_ends(c: &fantoccini::Client, suffix: &str) {
    let started = std::time::Instant::now();
    loop {
        if c.current_url().await.unwrap().as_str().ends_with(suffix) {
            return;
        }
        assert!(
            started.elapsed() <= Duration::from_secs(5),
            "URL never reached {suffix}",
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// The site host, against the same KIND
/// dependency tier, and the talks are mounted there. It must be explicit:
/// falling back to `NAV_BASE_URL` would drive Neon, which answers `404` for
/// every `/presentations` path.
const SITE_BASE_URL_ENV: &str = "NAV_BASE_URL";

/// Return the site's public origin. A local developer without the second
/// host can still run the rest of the suite; CI always sets
/// `NAV_REQUIRE_HARNESS=1`, so a missing endpoint is a hard failure.
fn site_base_url_or_skip() -> Option<String> {
    match env::var(SITE_BASE_URL_ENV) {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ if require_harness() => panic!(
            "NAV_REQUIRE_HARNESS=1 but {SITE_BASE_URL_ENV} is unset: \
             refusing to pass the Catalog deck test without its live host"
        ),
        _ => {
            eprintln!(
                "skipping the Catalog deck test: set {SITE_BASE_URL_ENV} to the live host origin"
            );
            None
        }
    }
}

/// The Catalog classroom step page navigates on a bare ArrowLeft/ArrowRight,
/// but Shift+Arrow (a text selection) and an arrow aimed at the Sections
/// disclosure must be left alone — regressions Greptile flagged on the
/// per-browser navigation change.
///
/// This is also the only check anywhere that `catalog-display.js` actually
/// reaches the page: the step markup renders identically with or without it,
/// so a missing hoist fails here as a navigation timeout and nowhere else.
#[tokio::test]
async fn catalog_step_arrow_keys_navigate_but_shift_and_focused_controls_do_not() {
    let Some(site_base_url) = site_base_url_or_skip() else {
        return;
    };
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    let deck = format!("{site_base_url}/presentations/rust-in-peace");

    // Plain ArrowRight advances one slide; ArrowLeft comes back.
    c.goto(&format!("{deck}/step/10")).await.unwrap();
    press_chord(&c, &[Key::Right.into()]).await;
    wait_url_ends(&c, "/step/11").await;
    press_chord(&c, &[Key::Left.into()]).await;
    wait_url_ends(&c, "/step/10").await;

    // Shift+ArrowRight extends a text selection on the reading page — it must
    // never move the deck.
    press_chord(&c, &[Key::Shift.into(), Key::Right.into()]).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        c.current_url()
            .await
            .unwrap()
            .as_str()
            .ends_with("/step/10"),
        "Shift+ArrowRight must not navigate the deck",
    );

    // An arrow key while the Sections disclosure is focused belongs to that
    // control, not the deck.
    c.execute(
        "document.querySelector('.workshop-rail .workshop-sections__toggle').focus();",
        vec![],
    )
    .await
    .unwrap();
    press_chord(&c, &[Key::Right.into()]).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        c.current_url()
            .await
            .unwrap()
            .as_str()
            .ends_with("/step/10"),
        "an arrow with the Sections button focused must not navigate the deck",
    );

    c.close().await.unwrap();
}

/// The public marketing surface at a phone width — no page renders wider than
/// its own viewport.
///
/// `/navigator` shipped a CSS Grid blowout: `.public-shell` sets
/// `grid-template-rows` but left its column implicit, so the column's
/// automatic minimum size took the min-content width of the page's
/// `<pre>` Homebrew commands (unbreakable by `white-space: pre`) and grew the
/// whole shell — header and footer included — past a 375px viewport. Every
/// practice-skin page (`/litigation`, `/fractional-gc`, `/fractional-cto`,
/// `/services`) carried a second, unrelated defect: their hero's decorative
/// glow bleeds `-25vw` past each edge on purpose, and the hero it bleeds from
/// carried no `overflow: hidden` to clip that bleed back to real layout
/// width. Both are one-line CSS fixes; this is the regression gate for
/// either recurring, on the pages that actually carried them plus a couple of
/// neighbors that share the same shell and hero.
#[tokio::test]
async fn public_marketing_pages_have_no_horizontal_overflow_on_mobile() {
    let Some(c) = new_client_or_skip().await else {
        return;
    };
    c.set_window_size(375, 812).await.unwrap();

    for path in [
        "/",
        "/navigator",
        "/litigation",
        "/fractional-gc",
        "/fractional-cto",
        "/services",
    ] {
        c.goto(&format!("{}{path}", base_url())).await.unwrap();
        wait_for_text(&c, "Neon Law", Duration::from_secs(10)).await;
        let widths = c
            .execute(
                "return [document.documentElement.scrollWidth, \
                 document.documentElement.clientWidth];",
                vec![],
            )
            .await
            .unwrap();
        let widths = widths.as_array().expect("[scrollWidth, clientWidth]");
        let scroll_width = widths[0].as_u64().unwrap();
        let client_width = widths[1].as_u64().unwrap();
        assert!(
            scroll_width <= client_width,
            "{path} scrolls horizontally at a phone width: \
             scrollWidth={scroll_width} clientWidth={client_width}",
        );
    }

    c.close().await.unwrap();
}
