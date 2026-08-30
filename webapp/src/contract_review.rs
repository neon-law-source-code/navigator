//! Attorney review screen for an inbound contract review as a Dioxus component
//! (#956 Phase 4) — `/app/lawyer/contract-reviews/{id}`.
//!
//! The successor to the `views::pages::admin::contract_reviews` render. It
//! shows the machine-proposed findings for per-finding attorney action. There is
//! **no bulk-accept**: each finding is its own native `POST` form with an
//! explicit *Accept* and *Reject* submit; the risk summary is its own form; and
//! the whole review can only be approved once every finding has been acted on.
//!
//! Every mutation stays on its existing `POST` handler
//! (`…/findings/{idx}`, `…/summary`, `…/approve`, `…/reject`), reached through
//! native forms carrying the session CSRF token — no JavaScript. A refused
//! approve redirects back here with `?error=`, surfaced above the page.
//!
//! # Authorization
//!
//! Unchanged from the handler: the route lives under `/app/lawyer/*`, so embedded Rego policy's
//! `lawyer_tier` rule gates it, and the loader adds the per-matter row scope —
//! a client role, or a lawyer not disclosed to the project, gets the same
//! `404` (admin bypasses the project scope inside
//! `store::access::can_see_project_as_lawyer`).
//!
//! The reads run the same `store` calls the handler made; there is no
//! `/api` cluster for contract reviews' read side yet (the client-facing write
//! door is `POST /app/api/projects/{id}/contract-review`). When one lands (#866)
//! this loader moves onto it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The review screen's `?error=` flash (set by the approve handler's
/// redirect-on-refusal).
#[derive(Deserialize, Default)]
pub struct ContractReviewQuery {
    #[serde(default)]
    pub error: Option<String>,
}

/// One finding, as the attorney sees it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FindingRow {
    pub index: usize,
    pub clause_ref: String,
    pub deviation: String,
    pub severity: String,
    pub suggested_redline: String,
    pub attorney_note: String,
    pub accepted: bool,
    /// Whether a decision has been recorded for this finding yet.
    pub acted: bool,
}

/// The rendered review screen. `found` is false for a missing review or a
/// caller outside the matter's lawyer lens — the fail-closed not-found state.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ContractReviewView {
    pub review_id: String,
    pub found: bool,
    pub playbook_name: String,
    /// `pending` / `analyzed` / `approved` / `rejected`.
    pub status: String,
    pub notation_state: String,
    pub risk_summary: String,
    pub findings: Vec<FindingRow>,
    pub all_acted: bool,
    /// The review still takes edits (`analyzed` + the matter parked at
    /// `lawyer_review`).
    pub editable: bool,
    pub error: Option<String>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The request context the loader reads back from the portal router's injected
/// extensions and the query string: the viewer's tier, the session CSRF token
/// for the write forms, the `?error=` flash, and the session's linked person id
/// (the subject of the per-matter row scope).
#[cfg(feature = "server")]
struct ReviewContext {
    role: ViewerRole,
    csrf_token: String,
    error: Option<String>,
    person_id: Option<uuid::Uuid>,
}

#[cfg(feature = "server")]
async fn review_context() -> ReviewContext {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let error = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ContractReviewQuery>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::extract::Query(q)| q.error);
    let person_id = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::portal_project_list::PersonId>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::Extension(pid)| pid.0)
    .and_then(|raw| raw.parse::<uuid::Uuid>().ok());
    ReviewContext {
        role,
        csrf_token,
        error,
        person_id,
    }
}

/// The review, its notation, and its playbook, once the per-matter row scope has
/// admitted the caller.
#[cfg(feature = "server")]
struct Scoped {
    review: store::contract_reviews::ContractReview,
    notation: store::notations::Notation,
    playbook: store::playbooks::Playbook,
}

/// Load the review with its notation and playbook, enforcing the per-matter row
/// scope. `None` for a missing review, a missing notation or playbook, or a
/// caller who may not see the matter — every one of which is a `404`, not a
/// distinguishable error.
#[cfg(feature = "server")]
async fn load_scoped(
    surreal: &store::surreal::SurrealDb,
    review_id: uuid::Uuid,
    person_id: Option<uuid::Uuid>,
    store_role: store::persons::Role,
) -> Result<Option<Scoped>, String> {
    let Some(review) = store::contract_reviews::by_id(surreal, review_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let Some(notation) = store::notations::find_by_id(surreal, review.notation_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    // A lawyer not disclosed to the matter gets a 404, not a peek.
    if !store::access::can_see_project_as_lawyer(
        surreal,
        person_id,
        store_role,
        notation.project_id,
    )
    .await?
    {
        return Ok(None);
    }
    let Some(playbook) = store::playbooks::by_id(surreal, review.playbook_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    Ok(Some(Scoped {
        review,
        notation,
        playbook,
    }))
}

/// Load the attorney review screen for the `{id}` in the request path, enforcing
/// the same per-matter row scope the handler did.
#[server]
pub async fn get_contract_review() -> Result<ContractReviewView, ServerFnError> {
    let ReviewContext {
        role,
        csrf_token,
        error,
        person_id,
    } = review_context().await;
    let firm_name = crate::app_chrome::firm_name_from_context().await;

    let not_found = |review_id: String| {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        ContractReviewView {
            firm_name: firm_name.clone(),
            review_id,
            found: false,
            role,
            ..ContractReviewView::default()
        }
    };

    // A client never reaches a lawyer surface.
    if !role.is_lawyer_tier() {
        return Ok(not_found(String::new()));
    }
    let Ok(axum::extract::Path(review_id)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await
    else {
        return Ok(not_found(String::new()));
    };
    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };
    let id = review_id.to_string();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let map_err = |e: String| ServerFnError::new(e.clone());

    let Some(Scoped {
        review,
        notation,
        playbook,
    }) = load_scoped(&surreal, review_id, person_id, store_role)
        .await
        .map_err(map_err)?
    else {
        return Ok(not_found(id));
    };

    let findings = store::contract_reviews::findings_of(&review).unwrap_or_default();
    let acted = store::contract_reviews::acted_finding_indices(&surreal, notation.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let all_acted = (0..findings.len()).all(|i| acted.contains(&i));
    let rows = findings
        .iter()
        .enumerate()
        .map(|(index, f)| FindingRow {
            index,
            clause_ref: f.clause_ref.clone(),
            deviation: f.deviation.clone(),
            severity: f.severity.clone(),
            suggested_redline: f.suggested_redline.clone().unwrap_or_default(),
            attorney_note: f.attorney_note.clone().unwrap_or_default(),
            accepted: f.accepted,
            acted: acted.contains(&index),
        })
        .collect();

    let editable = review.status == store::contract_reviews::STATUS_ANALYZED
        && notation.state == "lawyer_review";
    Ok(ContractReviewView {
        firm_name,
        review_id: id,
        found: true,
        playbook_name: playbook.name,
        status: review.status,
        notation_state: notation.state,
        risk_summary: review.risk_summary.unwrap_or_default(),
        findings: rows,
        all_acted,
        editable,
        error,
        csrf_token,
        role,
    })
}

/// The status pill's tone class, mirroring the badge's colour mapping.
fn status_tone(status: &str) -> &'static str {
    match status {
        "approved" => "nav-status--success",
        "rejected" => "nav-status--muted",
        "analyzed" => "nav-status--warning",
        _ => "nav-status--neutral",
    }
}

/// The risk-summary card — an editable textarea on an open review, plain prose
/// once the review is closed.
fn risk_summary_card(view: &ContractReviewView) -> Element {
    let action = format!("/app/lawyer/contract-reviews/{}/summary", view.review_id);
    let fields = vec![crate::components::Field::textarea(
        "Risk summary",
        "risk_summary",
        view.risk_summary.clone(),
        4,
    )];
    rsx! {
        if view.editable {
            crate::components::FormCard {
                title: "Risk summary".to_string(),
                action,
                submit_label: "Save summary".to_string(),
                heading: crate::components::Heading::H2,
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
        } else {
            div { class: "nav-card contract-review-summary",
                div { class: "nav-card__body",
                    h2 { class: "contract-review-summary__title", "Risk summary" }
                    p { class: "contract-review-summary__text", "{view.risk_summary}" }
                }
            }
        }
    }
}

/// The decision pill for one finding.
fn decision_badge(finding: &FindingRow) -> Element {
    let (tone, label) = if !finding.acted {
        ("nav-status--warning", "Needs action")
    } else if finding.accepted {
        ("nav-status--success", "Accepted")
    } else {
        ("nav-status--muted", "Rejected")
    };
    rsx! {
        span { class: "nav-badge contract-review-decision {tone}", "{label}" }
    }
}

/// One finding card: the clause, the deviation, and — on an open review — its
/// own edit form with the explicit Accept and Reject submits.
fn finding_card(view: &ContractReviewView, finding: &FindingRow) -> Element {
    let action = format!(
        "/app/lawyer/contract-reviews/{}/findings/{}",
        view.review_id, finding.index
    );
    let severity_options = ["low", "medium", "high"];
    rsx! {
        div { class: "nav-card contract-review-finding",
            div { class: "nav-card__body",
                div { class: "contract-review-finding__head",
                    h3 { class: "contract-review-finding__clause", "{finding.clause_ref}" }
                    {decision_badge(finding)}
                }
                p { class: "contract-review-finding__deviation", "{finding.deviation}" }
                if view.editable {
                    form {
                        class: "nav-form admin-form contract-review-finding__form",
                        method: "post",
                        action: "{action}",
                        "aria-label": "Finding {finding.index}",
                        input { r#type: "hidden", name: "_csrf", value: "{view.csrf_token}" }
                        div { class: "nav-field",
                            label { class: "nav-label", r#for: "severity-{finding.index}", "Severity" }
                            select {
                                class: "nav-select",
                                id: "severity-{finding.index}",
                                name: "severity",
                                for value in severity_options {
                                    option {
                                        value: "{value}",
                                        selected: finding.severity == value,
                                        "{severity_label(value)}"
                                    }
                                }
                            }
                        }
                        div { class: "nav-field",
                            label { class: "nav-label", r#for: "redline-{finding.index}", "Suggested redline" }
                            // A `<textarea>` is RCDATA: the value must be its
                            // escaped inner HTML, not a `value` attribute (ignored)
                            // or a child text node (hydration comments leak in).
                            textarea {
                                class: "nav-input",
                                id: "redline-{finding.index}",
                                name: "suggested_redline",
                                rows: "2",
                                dangerous_inner_html: escape_rcdata(&finding.suggested_redline),
                            }
                        }
                        div { class: "nav-field",
                            label { class: "nav-label", r#for: "note-{finding.index}", "Attorney note" }
                            textarea {
                                class: "nav-input",
                                id: "note-{finding.index}",
                                name: "attorney_note",
                                rows: "2",
                                dangerous_inner_html: escape_rcdata(&finding.attorney_note),
                            }
                        }
                        div { class: "contract-review-finding__actions",
                            button {
                                class: "nav-btn nav-btn--primary",
                                r#type: "submit",
                                name: "decision",
                                value: "accept",
                                "Accept"
                            }
                            button {
                                class: "nav-btn nav-btn--secondary",
                                r#type: "submit",
                                name: "decision",
                                value: "reject",
                                "Reject"
                            }
                        }
                    }
                } else {
                    if !finding.suggested_redline.is_empty() {
                        p { class: "contract-review-finding__redline",
                            strong { "Suggested redline: " }
                            "{finding.suggested_redline}"
                        }
                    }
                    if !finding.attorney_note.is_empty() {
                        p { class: "contract-review-finding__note",
                            strong { "Attorney note: " }
                            "{finding.attorney_note}"
                        }
                    }
                }
            }
        }
    }
}

/// Escape a string for safe inclusion as `<textarea>` RCDATA content.
fn escape_rcdata(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;")
}

/// The severity `<option>` label for a stored severity token.
fn severity_label(severity: &str) -> &'static str {
    match severity {
        "low" => "Low",
        "high" => "High",
        _ => "Medium",
    }
}

/// The approve / reject decision bar. Approve is disabled until every finding
/// carries a recorded decision.
fn decision_bar(view: &ContractReviewView) -> Element {
    let approve = format!("/app/lawyer/contract-reviews/{}/approve", view.review_id);
    let reject = format!("/app/lawyer/contract-reviews/{}/reject", view.review_id);
    rsx! {
        div { class: "contract-review-actions",
            form {
                class: "nav-form admin-form",
                method: "post",
                action: "{approve}",
                "aria-label": "Approve the review",
                input { r#type: "hidden", name: "_csrf", value: "{view.csrf_token}" }
                button {
                    class: "nav-btn nav-btn--primary",
                    r#type: "submit",
                    disabled: !view.all_acted,
                    "Approve & deliver memo"
                }
            }
            form {
                class: "nav-form admin-form",
                method: "post",
                action: "{reject}",
                "aria-label": "Reject the review",
                input { r#type: "hidden", name: "_csrf", value: "{view.csrf_token}" }
                button { class: "nav-btn nav-btn--danger", r#type: "submit", "Reject review" }
            }
        }
        if !view.all_acted {
            p { class: "muted contract-review-gate",
                "Act on every finding (accept or reject) before approving."
            }
        }
    }
}

/// The whole review body for a loaded review.
fn review_body(view: &ContractReviewView) -> Element {
    let tone = status_tone(&view.status);
    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Contract review" }
        header { class: "page-header",
            h1 { "Contract review" }
            p { class: "muted",
                "Measured against playbook: "
                strong { "{view.playbook_name}" }
            }
        }
        p { span { class: "nav-badge contract-review-status {tone}", "{view.status}" } }
        if let Some(error) = view.error.as_ref() {
            p { class: "nav-flash nav-flash--danger", role: "alert", "{error}" }
        }
        if !view.editable {
            p { class: "nav-form-notice contract-review-locked", role: "status",
                "This review is {view.status} — it is no longer editable."
            }
        }
        {risk_summary_card(view)}
        h2 { class: "contract-review-findings__title", "Findings" }
        if view.findings.is_empty() {
            p { class: "contract-review-empty", "The analysis flagged no positions." }
        }
        for finding in view.findings.iter() {
            {finding_card(view, finding)}
        }
        if view.editable {
            {decision_bar(view)}
        }
    }
}

/// The attorney contract-review screen. Server-side rendered; every write is a
/// native `POST` form carrying the session CSRF token, so the page works without
/// JavaScript.
#[component]
pub fn LawyerContractReview() -> Element {
    let resource = use_server_future(get_contract_review)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "contract-review", p { "Failed to load the review." } }
            }
        }
        None => {
            return rsx! {
                main { id: "contract-review", p { "Loading…" } }
            }
        }
    };
    let role = view.role;

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if role.is_lawyer_tier() {
                a { class: "nav-link", href: "/app/lawyer", "Lawyer" }
            }
            if role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "contract-review", class: "nav-theme",
            if view.found {
                {review_body(&view)}
            } else {
                document::Title { "{view.firm_name} | Lawyer | Not found" }
                h1 { "Not found" }
                p { "No contract review is available at this address." }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{review_body, ContractReviewView, FindingRow};
    use crate::people::ViewerRole;

    const RID: &str = "00000000-0000-0000-0000-000000000007";

    fn finding(acted: bool, accepted: bool) -> FindingRow {
        FindingRow {
            index: 0,
            clause_ref: "§7.2 Liability".to_string(),
            deviation: "Liability is uncapped.".to_string(),
            severity: "high".to_string(),
            suggested_redline: "Add a mutual cap.".to_string(),
            attorney_note: String::new(),
            accepted,
            acted,
        }
    }

    fn view(findings: Vec<FindingRow>, all_acted: bool) -> ContractReviewView {
        ContractReviewView {
            firm_name: "Neon Law".to_string(),
            review_id: RID.to_string(),
            found: true,
            playbook_name: "Vendor MSA".to_string(),
            status: "analyzed".to_string(),
            notation_state: "lawyer_review".to_string(),
            risk_summary: "One high-severity deviation.".to_string(),
            findings,
            all_acted,
            editable: true,
            error: None,
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
        }
    }

    fn render(view: &ContractReviewView) -> String {
        dioxus_ssr::render_element(review_body(view))
    }

    #[test]
    fn editable_review_renders_per_finding_accept_reject_forms() {
        let html = render(&view(vec![finding(false, false)], false));
        assert!(html.contains(">Vendor MSA<"), "{html}");
        assert!(html.contains("§7.2 Liability"), "{html}");
        // Per-finding accept/reject submits, not a bulk action.
        assert!(html.contains(r#"value="accept""#), "{html}");
        assert!(html.contains(r#"value="reject""#), "{html}");
        assert!(
            html.contains(&format!(
                r#"action="/app/lawyer/contract-reviews/{RID}/findings/0""#
            )),
            "{html}"
        );
        assert!(html.contains(">Needs action<"), "{html}");
        assert!(html.contains(r#"name="_csrf""#), "{html}");
        assert!(html.contains(r#"value="TOK""#), "{html}");
    }

    #[test]
    fn every_write_form_keeps_the_admin_form_e2e_hook() {
        // `web/tests/accessibility_e2e.rs` scopes axe to `form.admin-form`.
        let html = render(&view(vec![finding(false, false)], false));
        assert!(html.contains("admin-form"), "{html}");
    }

    #[test]
    fn prefilled_textareas_use_escaped_inner_html_not_a_value_attribute() {
        // A `<textarea>` is RCDATA: a `value` attribute is ignored by browsers
        // and a child text node leaks Dioxus hydration comments into the box.
        let mut f = finding(false, false);
        f.suggested_redline = "Cap at <5% & mutual".to_string();
        let html = render(&view(vec![f], false));
        assert!(html.contains("Cap at &lt;5% &amp; mutual"), "{html}");
        assert!(
            !html.contains(r#"name="suggested_redline" rows="2" value="#),
            "{html}"
        );
    }

    #[test]
    fn approve_is_disabled_until_all_findings_acted() {
        let html = render(&view(vec![finding(false, false)], false));
        assert!(html.contains("Approve &#38; deliver memo"), "{html}");
        assert!(html.contains("disabled"), "{html}");
        assert!(html.contains("Act on every finding"), "{html}");
    }

    #[test]
    fn approve_is_enabled_when_every_finding_is_acted() {
        let html = render(&view(vec![finding(true, true)], true));
        assert!(html.contains(">Accepted<"), "{html}");
        assert!(html.contains("Approve &#38; deliver memo"), "{html}");
        assert!(!html.contains("Act on every finding"), "{html}");
    }

    #[test]
    fn closed_review_is_read_only() {
        let mut v = view(vec![finding(true, true)], true);
        v.status = "approved".to_string();
        v.notation_state = "END".to_string();
        v.editable = false;
        let html = render(&v);
        assert!(html.contains("no longer editable"), "{html}");
        // No accept/reject submits and no write forms in read-only mode.
        assert!(!html.contains(r#"value="accept""#), "{html}");
        assert!(!html.contains("admin-form"), "{html}");
    }

    #[test]
    fn the_error_flash_renders_above_the_review() {
        let mut v = view(vec![finding(false, false)], false);
        v.error = Some("Every finding must be accepted or rejected.".to_string());
        let html = render(&v);
        assert!(html.contains("nav-flash--danger"), "{html}");
        assert!(
            html.contains(">Every finding must be accepted or rejected.<"),
            "{html}"
        );
    }

    #[test]
    fn an_empty_analysis_says_so() {
        let html = render(&view(Vec::new(), true));
        assert!(
            html.contains("The analysis flagged no positions."),
            "{html}"
        );
    }
}
