//! The notation review-and-send screen as a Dioxus component (#956 Phase 4) —
//! where an attorney reads the assembled document and decides.
//!
//! The successor to the `views::pages::admin::retainers::result`. This is
//! the page a **binding** envelope goes out from, so the three phases it keys
//! off stay exactly as they were:
//!
//! | condition | phase |
//! | --- | --- |
//! | no envelope, state `lawyer_review` | awaiting attorney review — approve, or request changes |
//! | no envelope, state `generate_pdf__*` | approved; document rendering — send |
//! | state `reask__client` | sent back for changes — re-collect |
//! | otherwise | intake started |
//!
//! No document is generated until a person approves, and the envelope goes out
//! only on the deliberate send. Both remain separate `POST`s.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// One questionnaire question the "Request changes" panel offers to flag.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ReaskQuestion {
    /// The state code — the checkbox posts as `q:{code}`.
    pub code: String,
    pub label: String,
}

/// The review screen, resolved by the portal pre-layer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct IntakeReviewData {
    pub notation_id: String,
    pub workflow_state: String,
    /// Present once an envelope has gone out. Its presence is half of the
    /// phase decision, so it is never inferred from the state alone.
    pub signature_request_id: Option<String>,
    /// The assembled document, rendered to HTML — what actually gets signed.
    pub rendered_html: String,
    pub reask_questions: Vec<ReaskQuestion>,
    /// The catalog string for the approve-and-send action, resolved server-side
    /// so the button and its accessible name cannot drift apart.
    pub approve_send_label: String,
}

/// The resolved review screen the portal pre-layer injects per request.
#[derive(Clone, Default)]
pub struct InjectedIntakeReview(pub IntakeReviewData);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct IntakeReviewView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub data: IntakeReviewData,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Read the injected review data, the viewer's tier and the session CSRF token.
/// The database, storage and template-assembly work already happened in the
/// portal pre-layer, so this loader only assembles them.
#[server]
pub async fn get_intake_review() -> Result<IntakeReviewView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let InjectedIntakeReview(data) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedIntakeReview>,
        _,
    >()
    .await
    .map(|axum::Extension(injected)| injected)
    .unwrap_or_default();
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    Ok(IntakeReviewView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        data,
        csrf_token,
        role,
    })
}

/// The notation review-and-send screen.
#[component]
pub fn IntakeReview() -> Element {
    let resource = use_server_future(get_intake_review)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "intake-review", p { "Failed to load this notation." } }
            }
        }
        None => {
            return rsx! {
                main { id: "intake-review", p { "Loading…" } }
            }
        }
    };
    review_body(&view)
}

/// Which of the four phases the notation is in. Keyed off the workflow state
/// **and** whether an envelope has gone out — a state alone cannot tell them
/// apart once one has.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingReview,
    ReadyToSend,
    SentBack,
    Started,
}

impl Phase {
    fn of(workflow_state: &str, signature_request_id: Option<&str>) -> Self {
        let no_envelope = signature_request_id.is_none();
        if no_envelope && workflow_state == "lawyer_review" {
            Self::AwaitingReview
        } else if no_envelope && workflow_state.starts_with("generate_pdf__") {
            Self::ReadyToSend
        } else if workflow_state == "reask__client" {
            Self::SentBack
        } else {
            Self::Started
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::AwaitingReview => "Awaiting attorney review",
            Self::ReadyToSend => "Document rendering — ready to send",
            Self::SentBack => "Sent back for changes",
            Self::Started => "Retainer intake started",
        }
    }
}

/// The loaded screen. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn review_body(view: &IntakeReviewView) -> Element {
    let data = &view.data;
    let phase = Phase::of(&data.workflow_state, data.signature_request_id.as_deref());
    let notation_id = data.notation_id.clone();
    let approve_send = format!("/app/lawyer/notations/{notation_id}/approve-send");
    let send = format!("/app/lawyer/notations/{notation_id}/send");
    let request_changes = format!("/app/lawyer/notations/{notation_id}/request-changes");
    let reask = format!("/app/lawyer/notations/{notation_id}/reask");
    let role = view.role;
    let csrf = view.csrf_token.clone();
    let signature = data.signature_request_id.clone().unwrap_or("—".to_string());
    let approve_label = data.approve_send_label.clone();
    let offers_reask = !data.reask_questions.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Notations | Review" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
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
        main { id: "intake-review", class: "nav-theme",
            h1 { {phase.heading()} }
            if phase == Phase::AwaitingReview {
                p { class: "nav-muted",
                    "Intake is complete and the matter parks here for an attorney — no "
                    "document is generated until a person approves. The exact document "
                    "below is what gets signed: review it, then approve and send."
                }
            }
            if phase == Phase::ReadyToSend {
                p { class: "nav-muted",
                    "Approved. The document is rendering for signature. Once it is ready, "
                    "send it — the binding envelope goes out only on this deliberate step."
                }
            }
            dl { class: "detail-dl",
                dt { "Notation id" }
                dd { "{notation_id}" }
                dt { "Workflow state" }
                dd { code { "{data.workflow_state}" } }
                dt { "Signature request" }
                dd { code { "{signature}" } }
            }
            if phase == Phase::SentBack {
                p { class: "nav-muted",
                    "The flagged answers were sent back for changes. Re-collect them on the "
                    "re-ask page — on the client's behalf, or let the client correct them "
                    "from their portal — then resubmit for review."
                }
                p {
                    a { class: "nav-btn nav-btn--primary", href: "{reask}",
                        "Re-collect flagged answers"
                    }
                }
            }
            if phase == Phase::AwaitingReview {
                form { method: "post", action: "{approve_send}", "aria-label": "{approve_label}",
                    input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                    button { class: "nav-btn nav-btn--primary", r#type: "submit", "{approve_label}" }
                }
                if offers_reask {
                    {request_changes_panel(&request_changes, &csrf, &data.reask_questions)}
                }
            }
            if phase == Phase::ReadyToSend {
                form { method: "post", action: "{send}", "aria-label": "Send for signature",
                    input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                    button { class: "nav-btn nav-btn--primary", r#type: "submit",
                        "Send for signature"
                    }
                }
            }
            h2 { "Rendered document" }
            // The assembled document, already rendered and sanitized by
            // `views::notation`. This is the artifact that gets signed, so it is
            // emitted as HTML rather than escaped into visible tags.
            div { dangerous_inner_html: "{data.rendered_html}" }
            p {
                a { href: "/app/lawyer/retainers/new", "Start another intake" }
            }
        }
    }
}

/// The "request changes instead" alternative to approving: flag the wrong
/// answers and send the matter back to re-collect them.
///
/// Declining the matter is a separate action — this one keeps it open, which is
/// why the panel is a disclosure under approve rather than a peer of it.
fn request_changes_panel(action: &str, csrf: &str, questions: &[ReaskQuestion]) -> Element {
    rsx! {
        details {
            summary { "Request changes instead" }
            p { class: "nav-muted",
                "Flag the answers that are wrong and send the matter back to "
                "re-collect them. This does not end the matter — declining is a "
                "separate action."
            }
            form {
                class: "nav-form",
                method: "post",
                action: "{action}",
                "aria-label": "Request changes to flagged answers",
                input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                for question in questions.iter() {
                    div { class: "nav-field nav-field--check",
                        input {
                            class: "nav-checkbox",
                            r#type: "checkbox",
                            id: "q-{question.code}",
                            name: "q:{question.code}",
                            value: "on",
                        }
                        label { class: "nav-label", r#for: "q-{question.code}",
                            "{question.label}"
                        }
                    }
                }
                div { class: "nav-field",
                    label { class: "nav-label", r#for: "reask-note",
                        "Note for the re-collection (optional)"
                    }
                    textarea { class: "nav-input", id: "reask-note", name: "note", rows: "2" }
                }
                button { class: "nav-btn nav-btn--secondary", r#type: "submit",
                    "Send back for changes"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTATION: &str = "00000000-0000-0000-0000-00000000002a";

    fn view(state: &str, signature: Option<&str>) -> IntakeReviewView {
        IntakeReviewView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            data: IntakeReviewData {
                notation_id: NOTATION.to_string(),
                workflow_state: state.to_string(),
                signature_request_id: signature.map(str::to_string),
                rendered_html: "<article class=\"notation\"><p>body</p></article>".to_string(),
                reask_questions: vec![ReaskQuestion {
                    code: "person__client".to_string(),
                    label: "Client name".to_string(),
                }],
                approve_send_label: "Approve and send for signature".to_string(),
            },
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
        }
    }

    fn render(view: &IntakeReviewView) -> String {
        dioxus_ssr::render_element(review_body(view))
    }

    #[test]
    fn the_screen_names_the_notation_state_and_signature_request() {
        let html = render(&view("sent_for_signature__pending", Some("stub-42-1")));
        assert!(html.contains(NOTATION), "{html}");
        assert!(html.contains("sent_for_signature__pending"), "{html}");
        assert!(html.contains("stub-42-1"), "{html}");
        // The assembled document is what gets signed, so it renders as markup.
        assert!(
            html.contains("<article class=\"notation\"><p>body</p></article>"),
            "{html}"
        );
    }

    #[test]
    fn awaiting_review_offers_approve_and_the_request_changes_panel() {
        let html = render(&view("lawyer_review", None));
        assert!(html.contains("Awaiting attorney review"), "{html}");
        assert!(
            html.contains(&format!(
                "action=\"/app/lawyer/notations/{NOTATION}/approve-send\""
            )),
            "{html}"
        );
        // The flaggable questions post under `q:{code}`, which is what
        // `request_changes_post` parses.
        assert!(html.contains("name=\"q:person__client\""), "{html}");
        assert!(html.contains("name=\"note\""), "{html}");
        // Nothing dispatches an envelope from this phase.
        assert!(
            !html.contains(&format!("action=\"/app/lawyer/notations/{NOTATION}/send\"")),
            "an unapproved notation must not offer send: {html}"
        );
    }

    #[test]
    fn only_the_rendering_phase_offers_the_send_that_dispatches_the_envelope() {
        let html = render(&view("generate_pdf__retainer_pdf", None));
        assert!(
            html.contains("Document rendering — ready to send"),
            "{html}"
        );
        assert!(
            html.contains(&format!("action=\"/app/lawyer/notations/{NOTATION}/send\"")),
            "{html}"
        );
        // Approval already happened; it is not offered twice.
        assert!(
            !html.contains(&format!(
                "action=\"/app/lawyer/notations/{NOTATION}/approve-send\""
            )),
            "{html}"
        );
    }

    #[test]
    fn an_envelope_already_out_offers_neither_approve_nor_send() {
        // The phase decision reads the signature request, not the state alone —
        // otherwise a `generate_pdf__*` state that already dispatched would
        // offer to dispatch again.
        let html = render(&view("generate_pdf__retainer_pdf", Some("stub-42-1")));
        for action in ["approve-send", "send"] {
            assert!(
                !html.contains(&format!(
                    "action=\"/app/lawyer/notations/{NOTATION}/{action}\""
                )),
                "a dispatched notation must not offer {action}: {html}"
            );
        }
    }

    #[test]
    fn a_notation_sent_back_points_at_the_re_ask_page() {
        let html = render(&view("reask__client", None));
        assert!(html.contains("Sent back for changes"), "{html}");
        assert!(
            html.contains(&format!("href=\"/app/lawyer/notations/{NOTATION}/reask\"")),
            "{html}"
        );
        assert!(
            !html.contains(&format!(
                "action=\"/app/lawyer/notations/{NOTATION}/approve-send\""
            )),
            "{html}"
        );
    }

    #[test]
    fn a_notation_with_no_flaggable_questions_hides_the_request_changes_panel() {
        let mut v = view("lawyer_review", None);
        v.data.reask_questions.clear();
        let html = render(&v);
        assert!(
            html.contains("approve-send"),
            "approve still offered: {html}"
        );
        assert!(!html.contains("Request changes instead"), "{html}");
    }

    #[test]
    fn every_write_on_the_page_carries_the_session_csrf_token() {
        let html = render(&view("lawyer_review", None));
        // approve-send + request-changes.
        assert_eq!(
            html.matches("name=\"_csrf\" value=\"TOK\"").count(),
            2,
            "{html}"
        );
    }
}
