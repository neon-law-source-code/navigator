//! The shared public lead-capture form.
//!
//! The page copy is resolved by the publishing brand and arrives here as a
//! small wasm-safe value. The portal supplies the signed double-submit token
//! and the source path through the request-extension seam.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// The copy the brand shows next to a lead form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LeadCaptureCopy {
    pub consent_sentence: String,
    pub phone_helper: String,
}

/// The per-request values the portal supplies to a public page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LeadCaptureContext {
    pub csrf_token: String,
    pub source_path: String,
}

/// Render a public lead form beside the page's ordinary mail action.
#[component]
pub fn LeadCaptureForm(copy: LeadCaptureCopy, context: LeadCaptureContext) -> Element {
    rsx! {
        form {
            class: "lead-capture-form",
            method: "post",
            action: "/leads",
            label {
                r#for: "lead-email",
                "Email",
                input {
                    id: "lead-email",
                    name: "email",
                    r#type: "email",
                    required: true,
                    maxlength: "254",
                    autocomplete: "email",
                }
            }
            label {
                r#for: "lead-phone",
                "Mobile phone",
                input {
                    id: "lead-phone",
                    name: "phone",
                    r#type: "tel",
                    maxlength: "32",
                    autocomplete: "tel",
                }
            }
            p { class: "lead-capture-form__helper",
                if let Some((before, after)) = copy.phone_helper.split_once("text-messaging terms") {
                    "{before}"
                    a { href: "/terms", "text-messaging terms" }
                    "{after}"
                } else {
                    "{copy.phone_helper}"
                }
            }
            label { class: "lead-capture-form__sms",
                input {
                    name: "sms_consent",
                    r#type: "checkbox",
                    value: "on",
                }
                "You may text me about this inquiry"
            }
            p { class: "lead-capture-form__consent",
                if let Some((before, after)) = copy.consent_sentence.split_once("Privacy Policy") {
                    "{before}"
                    a { href: "/privacy", "Privacy Policy" }
                    "{after}"
                } else {
                    "{copy.consent_sentence}"
                }
            }
            div { class: "nav-visually-hidden", aria_hidden: "true",
                label {
                    "Leave this field blank"
                    input {
                        name: "website",
                        tabindex: "-1",
                        autocomplete: "off",
                    }
                }
            }
            input { r#type: "hidden", name: "csrf_token", value: "{context.csrf_token}" }
            input { r#type: "hidden", name: "source_path", value: "{context.source_path}" }
            input { r#type: "hidden", name: "consent_version", value: "{copy.consent_sentence}" }
            button { r#type: "submit", "Send" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                        phone_helper: "Optional. If you add a mobile number and check the box, Neon Law may text you about this inquiry. Message and data rates may apply. Reply STOP to stop, HELP for help. See the text-messaging terms.".to_string(),
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
            "lead-capture-form",
            "name=\"email\"",
            "name=\"phone\"",
            "name=\"sms_consent\"",
            "name=\"website\"",
            "value=\"csrf-token\"",
            "value=\"/services\"",
            "href=\"/privacy\"",
            "href=\"/terms\"",
            ">Send<",
        ] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
    }
}
