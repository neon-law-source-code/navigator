//! What happens when a questionnaire reaches END: the notation's workflow
//! begins.
//!
//! That sentence is the rule, and this module is the seam that makes it true
//! wherever the last answer was recorded. Three doors advance the same
//! questionnaire runtime — the lawyer's form walk, the REST command
//! boundary, and Navigator MCP's `answer_notation` over A2A — and the drive they
//! hand off to lives in `portal`, because beginning a closing letter's
//! workflow means rendering one and `workflows` has no Typst compiler, no
//! storage bucket, and no business acquiring either.
//!
//! So the drive is declared here and implemented there. A host that has one
//! passes it in ([`mcp::McpState`]'s field, the same way it is handed its
//! storage and its mailer); a host that has none — a test fixture exercising
//! read-only tools — leaves it `None`, and the caller reports that the
//! questionnaire is complete but its workflow has not been started, rather
//! than claiming a start that never happened.
//!
//! The alternative, each door remembering to fire the transitions itself, is
//! what this replaces: the form walk did and the other two did not, so the
//! same completed questionnaire left the notation at `lawyer_review` or at
//! no machine at all depending on which door the answer came through.

use async_trait::async_trait;
use uuid::Uuid;

/// Why beginning a notation's workflow failed.
///
/// One opaque variant on purpose: the implementation's own error type is
/// rich (a missing template, an unparseable spec, a runtime that refused a
/// signal, a document that would not render), and every one of those is a
/// `500` to every caller here. Flattening to the message keeps this crate
/// from re-declaring an error surface it cannot act on.
#[derive(Debug, thiserror::Error)]
#[error("beginning the post-questionnaire workflow: {0}")]
pub struct PostQuestionnaireError(pub String);

/// Begin the workflow of a notation whose questionnaire has just reached
/// END.
///
/// Implemented once, by the host that owns document rendering. The returned
/// string is the state the notation now sits in — `lawyer_review` for a
/// template that stops at the human gate, `END` for one that runs to
/// completion — so a caller can report what it started rather than only
/// that it started something.
#[async_trait]
pub trait PostQuestionnaireDrive: Send + Sync {
    /// `acting` attributes every transition this fires to a Person, so the
    /// `notation_events` journal records who acted; `None` only for a host
    /// helper that truly has no authenticated individual, which the journal
    /// flags rather than silently attributing the action to the notation's
    /// client. Navigator MCP always resolves and supplies its caller.
    async fn begin(
        &self,
        notation_id: Uuid,
        acting: Option<Uuid>,
    ) -> Result<String, PostQuestionnaireError>;
}
