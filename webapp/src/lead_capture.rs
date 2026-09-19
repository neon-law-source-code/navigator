//! The shared public lead-capture form.
//!
//! The page copy is resolved by the publishing brand and arrives here as a
//! small wasm-safe value. The portal supplies the signed double-submit token
//! and the source path through the request-extension seam.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{CopyRun, Field, FormCard, Heading, Honeypot};

/// The effective-date identifier carried with the linked SMS policy.
pub const SMS_POLICY_VERSION: &str = "2026-09-18";

const SMS_PHONE_HELPER: &str = "Optional. Message frequency varies. Message and data rates may apply. Reply STOP to opt out or HELP for help. Our Privacy Policy and texting terms explain how we text and what we keep.";
const SMS_LABEL: &str = "Yes, {site_name} may send me text messages about this inquiry at this number, including automated texts. Texting is not a condition of hiring the firm.";

/// The copy the brand shows next to a lead form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LeadCaptureCopy {
    pub consent_sentence: String,
    pub phone_helper: String,
    pub sms_label: String,
}

impl LeadCaptureCopy {
    /// The exact SMS wording the form renders, as one evidence value.
    #[must_use]
    pub fn sms_consent_version(&self) -> String {
        format!("{}\n{}", self.phone_helper, self.sms_label)
    }
}

/// The per-request values the portal supplies to a public page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LeadCaptureContext {
    pub csrf_token: String,
    pub source_path: String,
}

/// Derive the current SMS evidence from server-owned copy and policy data.
///
/// The public form receives its copy from the same catalog values. This
/// server-side seam is what the submission handler uses; no requester field is
/// accepted as evidence.
#[must_use]
pub fn server_sms_consent_version(site_name: &str) -> String {
    format!(
        "{SMS_PHONE_HELPER}\n{}",
        SMS_LABEL.replace("{site_name}", site_name)
    )
}

/// Render a public lead form beside the page's ordinary mail action.
#[component]
pub fn LeadCaptureForm(copy: LeadCaptureCopy, context: LeadCaptureContext) -> Element {
    rsx! {
        FormCard {
            title: "Lead capture".to_string(),
            action: "/leads".to_string(),
            submit_label: "Send".to_string(),
            heading: Heading::Hidden,
            csrf_token: Some(context.csrf_token),
            fields: vec![
                Field::email("Email", "email", "")
                    .required()
                    .autocomplete("email")
                    .maxlength(254),
                Field::input("Mobile phone", "phone", "", "tel")
                    .autocomplete("tel")
                    .maxlength(32)
                    .help_runs(linked_runs(
                        &copy.phone_helper,
                        "Privacy Policy and texting terms",
                        "/privacy#text-messaging-sms",
                    )),
                Field::checkbox(copy.sms_label.clone(), "sms_consent", "on", false)
                    .help_runs(linked_runs(
                        &copy.consent_sentence,
                        "Privacy Policy",
                        "/privacy",
                    )),
            ],
            extra_fields: Some(rsx! {
                Honeypot { name: "website".to_string() }
                input { r#type: "hidden", name: "source_path", value: "{context.source_path}" }
                input { r#type: "hidden", name: "consent_version", value: "{copy.consent_sentence}" }
            }),
        }
    }
}

fn linked_runs(text: &str, token: &str, href: &str) -> Vec<CopyRun> {
    let Some((before, after)) = text.split_once(token) else {
        return vec![CopyRun {
            text: text.to_string(),
            emphasis: false,
            href: None,
        }];
    };
    let mut runs = Vec::with_capacity(3);
    if !before.is_empty() {
        runs.push(CopyRun {
            text: before.to_string(),
            emphasis: false,
            href: None,
        });
    }
    runs.push(CopyRun {
        text: token.to_string(),
        emphasis: false,
        href: Some(href.to_string()),
    });
    if !after.is_empty() {
        runs.push(CopyRun {
            text: after.to_string(),
            emphasis: false,
            href: None,
        });
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::assert_forms_accessible;

    fn render(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn renders_the_form_copy_links_and_double_submit_fields() {
        fn app() -> Element {
            rsx! {
                LeadCaptureForm {
                    copy: LeadCaptureCopy {
                        consent_sentence: "By sending this, you agree that Neon Law may email you about this inquiry. Sending it does not make you a client, and nothing on this page is legal advice. See our Privacy Policy.".to_string(),
                        phone_helper: "Optional. Message frequency varies. Message and data rates may apply. Reply STOP to opt out or HELP for help. Our Privacy Policy and texting terms explain how we text and what we keep.".to_string(),
                        sms_label: "Yes, Neon Law may send me text messages about this inquiry at this number, including automated texts. Texting is not a condition of hiring the firm.".to_string(),
                    },
                    context: LeadCaptureContext {
                        csrf_token: "csrf-token".to_string(),
                        source_path: "/services".to_string(),
                    },
                }
            }
        }

        let html = render(app);
        for expected in [
            "nav-form",
            "name=\"email\"",
            "name=\"phone\"",
            "name=\"sms_consent\"",
            "name=\"website\"",
            "value=\"csrf-token\"",
            "value=\"/services\"",
            "href=\"/privacy\"",
            "href=\"/privacy#text-messaging-sms\"",
            "class=\"nav-input\"",
            "class=\"nav-checkbox\"",
            "class=\"nav-btn nav-btn--primary\"",
            ">Send<",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
        assert!(html.contains(r#"aria-label="Lead capture""#), "{html}");
        assert!(!html.contains("sms_consent_version"), "{html}");
        assert!(!html.contains("sms_policy_version"), "{html}");
        assert_forms_accessible(&html, "lead form");
    }
}
