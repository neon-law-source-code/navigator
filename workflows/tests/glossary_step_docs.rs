//! Drift guard for workflow-prefix vocabulary in `docs/glossary/`.
//!
//! Every state-name prefix accepted by the workflow engine needs a glossary
//! entry with a link, so readers can jump from the term to the implementation
//! or authoring docs.

use workflows::step::STEP_PREFIXES;

#[test]
fn every_workflow_prefix_has_a_linked_glossary_entry() {
    for (prefix, _) in STEP_PREFIXES {
        let heading = glossary_heading_for_prefix(prefix);
        let section = glossary_section(heading)
            .unwrap_or_else(|| panic!("missing glossary term `{heading}` for `{prefix}`"));
        assert!(
            section.contains(']') && section.contains("]("),
            "`{heading}` must link to the authoring docs or source"
        );
        assert!(
            section.contains(&format!("`{prefix}`"))
                || (*prefix == "witnesses" && section.contains("Signature step kind")),
            "`{heading}` must mention the literal workflow prefix `{prefix}`"
        );
    }
}

fn glossary_section(heading: &str) -> Option<&'static str> {
    store::glossary::terms()
        .iter()
        .find(|term| term.title == heading)
        .map(|term| term.body.as_str())
}

fn glossary_heading_for_prefix(prefix: &str) -> &'static str {
    match prefix {
        "_signature" => "Signature",
        "analysis" => "Analysis",
        "certified_mail" => "Certified Mail",
        "client_review" => "Client Review",
        "document_drafts" => "Document Drafts",
        "document_intake" => "Document Intake",
        "generate_pdf" => "Document Open",
        "e_filing" => "E-Filing",
        "email_send" => "Email Send",
        "extract" => "Extract",
        "filing" => "Filing",
        "firm_signature" => "Firm Signature",
        "intake_persisted" => "Intake Persisted",
        "mailroom_receive" => "Mailroom Receive",
        "mailroom_send" => "Mailroom Send",
        "notarization" => "Notarization",
        "onchain" => "On-Chain Record",
        "reask" => "Re-ask",
        "sent_for_signature" => "Sent for Signature",
        "lawyer_review" => "Lawyer Review",
        "witnesses" => "Witnesses",
        other => panic!("add a glossary heading mapping for workflow prefix `{other}`"),
    }
}
