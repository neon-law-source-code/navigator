//! Canonical catalog of notation **workflow steps** — the one place
//! that says, for each step-name prefix, *what the step actually does*
//! and *how built it is*.
//!
//! This is the description source the `navigator-lsp` hover reads (so
//! highlighting a `workflow:` state in a template shows what that step
//! does, the way hovering a function shows its doc) and the allow-list
//! `N104` validates against. It mirrors the engine registry
//! `workflows::step::STEP_PREFIXES`; the drift test
//! `catalog_covers_every_engine_prefix` (below) fails the build if the
//! engine gains a step this catalog doesn't describe, so a new step
//! *forces* a description here — there is no such thing as an
//! undocumented workflow step.
//!
//! The summaries are grounded in the dispatch code, not aspiration:
//! `mailroom_send` *records a filings row*, it does not post mail;
//! `sent_for_signature` *waits for the e-signature webhook* while the
//! envelope is sent web-side. Keep them honest.

/// How built a workflow step is. Shown in the LSP hover so an author
/// can tell a real automation from a human gate or an external seam at
/// a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// Real worker code runs (or a wired wait state the runtime drives).
    Implemented,
    /// No worker code: an external system (Navigator MCP / Gemini) does the work
    /// off-process and signals the workflow to advance.
    Seam,
    /// A human acts (approve, sign, notarize); the step only records the
    /// decision. There is no automation to build here.
    Human,
    /// The step kind, dispatch arm, and durable table exist, but the
    /// real side effect is deferred behind a default no-op.
    Scaffolded,
}

impl StepStatus {
    /// Short human label for the LSP hover / reference page.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Implemented => "Implemented",
            Self::Seam => "External seam (Navigator MCP/Gemini)",
            Self::Human => "Human step — no worker automation",
            Self::Scaffolded => "Scaffolded — deferred",
        }
    }
}

/// One workflow step in the catalog.
#[derive(Debug, Clone, Copy)]
pub struct WorkflowStep {
    /// State-name prefix before the `__discriminator` (e.g.
    /// `generate_pdf`). `_signature` is the synthetic key for the
    /// respondent-signing *suffix* family (`member_signatures`, …).
    pub prefix: &'static str,
    /// How built the step is.
    pub status: StepStatus,
    /// What the step actually does, grounded in the dispatch code.
    pub summary: &'static str,
}

/// The catalog. One entry per `workflows::step::STEP_PREFIXES` prefix.
pub const WORKFLOW_STEPS: &[WorkflowStep] = &[
    WorkflowStep {
        prefix: "intake_persisted",
        status: StepStatus::Implemented,
        summary: "Pass-through wait state: the submitted intake answers are recorded on the \
                  workflow journal, then it advances. No document or external call.",
    },
    WorkflowStep {
        prefix: "lawyer_review",
        status: StepStatus::Human,
        summary: "Pauses for a lawyer (an attorney) to approve or reject. No automated side \
                  effect — the human decision drives the next transition. This is the mandatory \
                  legal gate every notation must cross.",
    },
    WorkflowStep {
        prefix: "client_review",
        status: StepStatus::Human,
        summary: "Pauses for the respondent (client) to read attorney-approved drafts and approve \
                  them on the review surface. Human-driven; no worker side effect.",
    },
    WorkflowStep {
        prefix: "reask",
        status: StepStatus::Human,
        summary: "A lawyer_review that returned changes_requested parks here so the flagged \
                  answers can be re-collected (the client self-serve, or lawyer on their behalf) \
                  before the matter returns to lawyer_review. Human-driven wait state; no worker \
                  side effect. Keeps a rejected review from dead-ending — `rejected` is reserved \
                  for a genuine withdrawal.",
    },
    WorkflowStep {
        prefix: "notarization",
        status: StepStatus::Human,
        summary: "The respondent signs or refuses before a notary. A human act recorded on the \
                  journal; no worker automation.",
    },
    WorkflowStep {
        prefix: "document_intake",
        status: StepStatus::Implemented,
        summary:
            "Files a provided artifact (a text paste, a file, or a link) into the matter as a \
                  content-addressed blob plus a documents row, via store::documents::ingest_bytes.",
    },
    WorkflowStep {
        prefix: "extract",
        status: StepStatus::Seam,
        summary:
            "External seam: Navigator MCP/Gemini mines the structured estate inputs out of an uploaded \
                  transcript off-process, then signals the workflow to advance. No worker code \
                  runs in this step.",
    },
    WorkflowStep {
        prefix: "analysis",
        status: StepStatus::Seam,
        summary: "External seam: web (Vertex Gemini) flags playbook deviations in an uploaded \
                  contract and persists the findings, then signals advance. No worker code runs \
                  in this step.",
    },
    WorkflowStep {
        prefix: "document_drafts",
        status: StepStatus::Implemented,
        summary: "web renders the generated instruments into review_documents rows; the \
                  orchestrator advances out when they land. A wait state, not a worker PDF \
                  dispatch.",
    },
    WorkflowStep {
        prefix: "generate_pdf",
        status: StepStatus::Implemented,
        summary: "Renders the template body to a PDF (Typst) — or fills a blank government \
                  AcroForm with the answers — and persists the bytes to object storage. \
                  Attorney-review-ready, never auto-filed.",
    },
    WorkflowStep {
        prefix: "sent_for_signature",
        status: StepStatus::Implemented,
        summary: "The assembled PDF is sent out for e-signature (a DocuSign envelope created \
                  web-side). The workflow waits here until the e-signature webhook reports the \
                  document signed or declined.",
    },
    WorkflowStep {
        prefix: "firm_signature",
        status: StepStatus::Human,
        summary: "A firm lawyer signs an outbound document (the canonical case is the \
                  closing letter that ends a matter). A human act recorded on the journal.",
    },
    WorkflowStep {
        prefix: "mailroom_send",
        status: StepStatus::Implemented,
        summary:
            "Records an outbound physical-mail submission as a filings row (proof of what was \
                  sent and to which office). The actual posting is a manual lawyer act — this step \
                  records it, it does not mail. Reached only after lawyer_review.",
    },
    WorkflowStep {
        prefix: "mailroom_receive",
        status: StepStatus::Human,
        summary: "Logs inbound physical mail (recorded by the SendGrid inbound webhook, not a \
                  worker step).",
    },
    WorkflowStep {
        prefix: "email_send",
        status: StepStatus::Implemented,
        summary: "Renders an email template (today only `welcome`) and sends it through the \
                  configured EmailService (SendGrid in prod). Advances when the provider accepts.",
    },
    WorkflowStep {
        prefix: "certified_mail",
        status: StepStatus::Implemented,
        summary: "Records a USPS certified-mail submission as a filings row (proof of mailing). \
                  The physical send is a manual lawyer act; this records it. After lawyer_review.",
    },
    WorkflowStep {
        prefix: "e_filing",
        status: StepStatus::Implemented,
        summary:
            "Records an electronic filing with a government office as a filings row. The \
                  submission itself is performed out of band; this records it. After lawyer_review.",
    },
    WorkflowStep {
        prefix: "filing",
        status: StepStatus::Implemented,
        summary: "Records a filing with a named government office (e.g. filing__nv_sos) as a \
                  filings row. The submission itself is performed out of band; this records it. \
                  After lawyer_review.",
    },
    WorkflowStep {
        prefix: "onchain",
        status: StepStatus::Scaffolded,
        summary:
            "Hashes the attested document and writes a durable attestations row. The on-chain \
                  write is deferred — the default NullAttestor records no transaction, so the row \
                  stays pending until a real chain backend is configured.",
    },
    WorkflowStep {
        prefix: "_signature",
        status: StepStatus::Human,
        summary: "The respondent (or their witnesses) sign inside the portal; the signature is \
                  recorded on the journal. Matches state names ending in `_signature` / \
                  `_signatures` (e.g. member_signatures). Human-driven; no worker side effect.",
    },
    WorkflowStep {
        prefix: "witnesses",
        status: StepStatus::Human,
        summary: "A respondent's witnesses sign (e.g. a will). Resolves to the respondent-signing \
                  step kind; human-driven, no worker side effect.",
    },
];

/// Look up the catalog entry for a workflow state name or bare prefix.
///
/// Strips a `__discriminator` suffix, then matches the literal prefix;
/// failing that, a name ending in `_signature` / `_signatures` resolves
/// to the synthetic `_signature` family entry (mirroring
/// `workflows::step::step_kind_for`). `BEGIN` / `END` and unknown
/// prefixes return `None`.
#[must_use]
pub fn lookup(state_or_prefix: &str) -> Option<&'static WorkflowStep> {
    let prefix = state_or_prefix
        .split_once("__")
        .map_or(state_or_prefix, |(p, _)| p);
    if let Some(step) = WORKFLOW_STEPS.iter().find(|s| s.prefix == prefix) {
        return Some(step);
    }
    if prefix.ends_with("_signature") || prefix.ends_with("_signatures") {
        return WORKFLOW_STEPS.iter().find(|s| s.prefix == "_signature");
    }
    None
}

/// True when `prefix` is a known, allowed workflow-step prefix.
#[must_use]
pub fn is_allowed_prefix(prefix: &str) -> bool {
    lookup(prefix).is_some()
}

#[cfg(test)]
mod tests {
    use super::{lookup, StepStatus, WORKFLOW_STEPS};

    #[test]
    fn every_step_has_a_real_summary() {
        for step in WORKFLOW_STEPS {
            assert!(
                step.summary.len() > 20 && !step.summary.contains('\n'),
                "{} needs a one-paragraph summary, got {:?}",
                step.prefix,
                step.summary,
            );
        }
    }

    #[test]
    fn lookup_strips_discriminator() {
        let s = lookup("generate_pdf__trust_pdf").expect("generate_pdf is in the catalog");
        assert_eq!(s.prefix, "generate_pdf");
        assert_eq!(s.status, StepStatus::Implemented);
        assert!(s.summary.contains("PDF"));
    }

    #[test]
    fn lookup_resolves_the_signature_suffix_family() {
        let s = lookup("member_signatures").expect("signature family resolves");
        assert_eq!(s.prefix, "_signature");
        assert_eq!(s.status, StepStatus::Human);
    }

    #[test]
    fn lookup_returns_none_for_markers_and_unknowns() {
        assert!(lookup("BEGIN").is_none());
        assert!(lookup("END").is_none());
        assert!(lookup("bespoke_magic").is_none());
    }

    #[test]
    fn retired_document_open_prefix_is_absent_from_the_catalog() {
        // The step was renamed `document_open` → `generate_pdf` and the
        // old prefix removed outright (no alias). The catalog must not
        // still describe it, or a notation could author a step the
        // dispatcher no longer knows.
        assert!(lookup("document_open").is_none());
        assert!(lookup("document_open__retainer_pdf").is_none());
        assert!(!super::is_allowed_prefix("document_open"));
    }

    #[test]
    fn mailroom_send_summary_is_honest_about_not_mailing() {
        // The whole point of the catalog: say what the step really does.
        let s = lookup("mailroom_send").unwrap();
        assert!(
            s.summary.contains("records") && s.summary.contains("does not mail"),
            "mailroom_send summary must be honest, got {:?}",
            s.summary,
        );
    }

    #[test]
    fn catalog_covers_every_engine_prefix() {
        // The drift guard: every prefix the workflow engine routes must
        // be described here. A new step in workflows::step forces a
        // description in this catalog — no undocumented steps.
        for (prefix, _) in workflows::step::STEP_PREFIXES {
            assert!(
                WORKFLOW_STEPS.iter().any(|s| s.prefix == *prefix),
                "engine step prefix `{prefix}` has no entry in the workflow-step catalog",
            );
        }
    }

    #[test]
    fn catalog_has_no_prefix_unknown_to_the_engine() {
        // And the reverse: the catalog must not describe a step the
        // engine doesn't route, or hover would lie.
        for step in WORKFLOW_STEPS {
            assert!(
                workflows::step::STEP_PREFIXES
                    .iter()
                    .any(|(p, _)| *p == step.prefix),
                "catalog prefix `{}` is not a real engine step",
                step.prefix,
            );
        }
    }
}
