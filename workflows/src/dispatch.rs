//! The step-dispatch registry — the single match from a transition's
//! landing state to its worker side effect.
//!
//! Before this module the same dispatch match lived in two places that
//! had to be kept in sync by hand: the prod worker
//! (`workflows-service::notation_service::workflow_signal`) and the
//! in-process dev/BDD runtime ([`crate::DispatchingRuntime`]). Every new
//! step kind meant two synchronized edits. [`dispatch_step`] is now the
//! one arm both callers share — adding a step is one match arm here.
//!
//! ## Durability stays the caller's
//!
//! This module is deliberately ignorant of Restate. Each dispatch is a
//! pure async side effect; the caller owns the `ctx.run` boundary:
//!
//! - the worker wraps [`dispatch_step`] in a single
//!   `ctx.run("dispatch-step", …)` so a replay reuses the journaled
//!   outcome instead of re-emailing / double-filing / double-writing;
//! - the in-process [`crate::DispatchingRuntime`] calls it inline.
//!
//! A registry that owned `ctx.run` itself would reintroduce the
//! duplicate-effect bug — so it doesn't. The worker uses
//! [`dispatches_side_effect`] to decide whether to open a journal entry
//! at all, so no-side-effect transitions (`lawyer_review`, `_signature`,
//! …) stay journal-free exactly as before.

use std::sync::Arc;

use uuid::Uuid;

use crate::attest::{dispatch_onchain_record, Attestor, OnChainPayload};
use crate::compliance::{dispatch_compliance, is_dispatched_submission, CompliancePayload};
use crate::document::{dispatch_generate_pdf, DocumentPayload};
use crate::email::{dispatch_state, EmailPayload, EmailService};
use crate::intake::{dispatch_document_intake, IntakePayload};
use crate::spec::StateName;
use crate::step::{step_kind_for, StepKind};

/// The providers a step dispatch may need — the same seams `AppState`
/// and the `workflows-service` worker already hold. Owned `Arc`s (not
/// borrows) so the worker can move a clone into its `ctx.run` closure,
/// which must be `'static`.
///
#[derive(Clone)]
pub struct StepDeps {
    pub email: Arc<dyn EmailService>,
    pub storage: Arc<dyn cloud::StorageService>,
    /// The SurrealDB handle. Optional because an `FsStorage` temp dir and
    /// a `CapturingEmail` are cheap while a store is not, so the email
    /// unit tests stay store-free. The document steps need it: a
    /// `generate_pdf` writes the asset row through it and reads the
    /// pinned template's declared kind from it. Set via
    /// [`StepDeps::with_surreal`]; a document step reached without one
    /// errors clearly rather than silently skipping the write.
    pub surreal: Option<store::surreal::SurrealDb>,
    /// On-chain attestor for the `onchain__record_attestation` step.
    /// Optional — like `surreal`, it is only needed by one step family, so
    /// callers that never reach an on-chain step (the email/document unit
    /// tests) leave it `None`. Set via [`StepDeps::with_attestor`]; an
    /// on-chain step reached without one errors clearly. The default
    /// production/dev attestor is [`crate::attest::NullAttestor`] (records
    /// no transaction), selected by `crate::attest::attestor_from_env`.
    pub attestor: Option<Arc<dyn Attestor>>,
}

impl StepDeps {
    #[must_use]
    pub fn new(email: Arc<dyn EmailService>, storage: Arc<dyn cloud::StorageService>) -> Self {
        Self {
            email,
            storage,
            surreal: None,
            attestor: None,
        }
    }

    /// Attach the SurrealDB handle the document steps need — `assets` and
    /// `templates` live there since ENG-121.
    #[must_use]
    pub fn with_surreal(mut self, surreal: store::surreal::SurrealDb) -> Self {
        self.surreal = Some(surreal);
        self
    }

    /// Attach an [`Attestor`] so the `onchain__record_attestation` step
    /// can record an on-chain attestation in-process. Required for any
    /// workflow that reaches an `onchain__*` step.
    #[must_use]
    pub fn with_attestor(mut self, attestor: Arc<dyn Attestor>) -> Self {
        self.attestor = Some(attestor);
        self
    }
}

/// Failure of a step dispatch. Both callers flatten this to their own
/// transport/terminal error via `to_string()`: the worker to a
/// `TerminalError`, the in-process runtime to
/// [`crate::WorkflowRuntimeError::Transport`].
#[derive(Debug, thiserror::Error)]
pub enum StepDispatchError {
    /// The signal carried no `value` but the step needs a payload.
    #[error("{0} step requires a payload threaded through the signal `value`")]
    MissingPayload(&'static str),
    /// The `value` payload didn't decode into the step's payload type.
    #[error("decode {what} payload: {source}")]
    Decode {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    /// A step was reached without the SurrealDB handle it needs.
    #[error("{0} step requires a SurrealDB handle (StepDeps::surreal)")]
    MissingSurreal(&'static str),
    /// An on-chain step was reached without an attestor configured.
    #[error("{0} step requires an attestor (StepDeps::with_attestor)")]
    MissingAttestor(&'static str),
    /// The underlying dispatch fn (email / document / compliance) failed.
    #[error("dispatch: {0}")]
    Dispatch(String),
}

/// Whether the transition landing on `next` produces a worker side
/// effect. The worker uses this to decide whether to open a `ctx.run`
/// journal entry at all — no-side-effect transitions (`lawyer_review`,
/// `notarization`, `_signature`, …) get none, as before this refactor.
#[must_use]
pub fn dispatches_side_effect(next: &StateName) -> bool {
    matches!(
        step_kind_for(next),
        Some(
            StepKind::EmailSend
                | StepKind::GeneratePdf
                | StepKind::DocumentIntake
                | StepKind::OnChainRecord
        )
    ) || is_dispatched_submission(next)
}

/// Run the side effect for the step the workflow just landed on.
///
/// One match arm per dispatched step kind; every other state is a no-op
/// `Ok(())`. The caller owns the `ctx.run` boundary, so journaling stays
/// outside the registry (see the module docs). Idempotent by
/// construction — each underlying dispatch writes the same bytes / row
/// for the same payload, so a Restate replay through `ctx.run` is safe.
///
/// The `Ok` value is an optional JSON payload the step produced for the
/// caller to journal on the transition's `notation_events` row — today
/// only the `generate_pdf` step returns one (a [`GeneratedPdfRef`] link
/// to the persisted blob); every other step returns `None`.
///
/// [`GeneratedPdfRef`]: crate::document::GeneratedPdfRef
pub async fn dispatch_step(
    deps: &StepDeps,
    notation_id: Uuid,
    next: &StateName,
    payload: Option<&str>,
) -> Result<Option<String>, StepDispatchError> {
    match step_kind_for(next) {
        Some(StepKind::EmailSend) => {
            let payload = decode::<EmailPayload>("email_send", payload)?;
            dispatch_state(deps.email.as_ref(), next.as_str(), &payload)
                .await
                .map(|_| None)
                .map_err(|e| StepDispatchError::Dispatch(e.to_string()))
        }
        Some(StepKind::GeneratePdf) => {
            let payload = decode::<DocumentPayload>("generate_pdf", payload)?;
            dispatch_generate_pdf(&deps.storage, deps.surreal.as_ref(), notation_id, &payload)
                .await
                .map_err(|e| StepDispatchError::Dispatch(e.to_string()))
        }
        Some(StepKind::DocumentIntake) => {
            let payload = decode::<IntakePayload>("document_intake", payload)?;
            let surreal = deps
                .surreal
                .as_ref()
                .ok_or(StepDispatchError::MissingSurreal("document_intake"))?;
            dispatch_document_intake(surreal, &deps.storage, notation_id, &payload)
                .await
                .map(|_| None)
                .map_err(|e| StepDispatchError::Dispatch(e.to_string()))
        }
        Some(StepKind::OnChainRecord) => {
            let payload = decode::<OnChainPayload>("onchain_record", payload)?;
            let attestor = deps
                .attestor
                .as_ref()
                .ok_or(StepDispatchError::MissingAttestor("onchain_record"))?;
            let surreal = deps
                .surreal
                .as_ref()
                .ok_or(StepDispatchError::MissingSurreal("onchain_record"))?;
            dispatch_onchain_record(
                deps.storage.as_ref(),
                attestor.as_ref(),
                surreal,
                notation_id,
                &payload,
            )
            .await
            .map(|()| None)
            .map_err(|e| StepDispatchError::Dispatch(e.to_string()))
        }
        _ if is_dispatched_submission(next) => {
            let payload = decode::<CompliancePayload>("compliance submission", payload)?;
            let surreal = deps
                .surreal
                .as_ref()
                .ok_or(StepDispatchError::MissingSurreal("compliance submission"))?;
            dispatch_compliance(surreal, notation_id, next, &payload)
                .await
                .map(|()| None)
                .map_err(|e| StepDispatchError::Dispatch(e.to_string()))
        }
        _ => Ok(None),
    }
}

/// Decode the signal `value` into a step payload, mapping the two
/// failure shapes (absent / malformed) to [`StepDispatchError`].
fn decode<T: serde::de::DeserializeOwned>(
    what: &'static str,
    payload: Option<&str>,
) -> Result<T, StepDispatchError> {
    let raw = payload.ok_or(StepDispatchError::MissingPayload(what))?;
    serde_json::from_str(raw).map_err(|source| StepDispatchError::Decode { what, source })
}

#[cfg(test)]
mod tests {
    use super::{dispatch_step, dispatches_side_effect, StepDeps, StepDispatchError};
    use crate::document::DocumentPayload;
    use crate::email::{CapturingEmail, EmailPayload, EmailService};
    use crate::spec::StateName;
    use std::sync::Arc;
    use uuid::Uuid;

    async fn fs_storage(suite: &str) -> Arc<dyn cloud::StorageService> {
        Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-registry-{suite}")))
                .await
                .expect("temp FsStorage"),
        )
    }

    fn deps_with(
        email: Arc<dyn EmailService>,
        storage: Arc<dyn cloud::StorageService>,
    ) -> StepDeps {
        StepDeps::new(email, storage)
    }

    #[test]
    fn dispatches_side_effect_marks_only_the_dispatched_families() {
        // EmailSend / GeneratePdf / DocumentIntake / the submission
        // family produce a worker side effect; human and wait states do
        // not.
        assert!(dispatches_side_effect(&StateName::from(
            "email_send__welcome"
        )));
        assert!(dispatches_side_effect(&StateName::from(
            "generate_pdf__retainer_pdf"
        )));
        assert!(dispatches_side_effect(&StateName::from(
            "document_intake__transcript"
        )));
        assert!(dispatches_side_effect(&StateName::from("mailroom_send")));
        assert!(dispatches_side_effect(&StateName::from("filing__nv_sos")));
        assert!(dispatches_side_effect(&StateName::from(
            "onchain__record_attestation"
        )));
        // No side effect — the worker must not open a ctx.run for these.
        assert!(!dispatches_side_effect(&StateName::from("lawyer_review")));
        assert!(!dispatches_side_effect(&StateName::from("client_review")));
        assert!(!dispatches_side_effect(&StateName::from(
            "testator_signature"
        )));
        assert!(!dispatches_side_effect(&StateName::from("extract__inputs")));
        assert!(!dispatches_side_effect(&StateName::end()));
    }

    #[tokio::test]
    async fn email_send_arm_dispatches_through_the_injected_service() {
        let email = Arc::new(CapturingEmail::new());
        let deps = deps_with(email.clone(), fs_storage("email").await);
        let payload =
            serde_json::to_string(&EmailPayload::new("Aries", "aries@example.com")).unwrap();

        dispatch_step(
            &deps,
            Uuid::from_u128(1),
            &StateName::from("email_send__welcome"),
            Some(&payload),
        )
        .await
        .expect("email_send dispatch succeeds");

        let captured = email.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].to, "aries@example.com");
        assert_eq!(captured[0].template_slug.as_deref(), Some("welcome"));
    }

    #[tokio::test]
    async fn generate_pdf_arm_renders_and_persists_through_storage() {
        let storage = fs_storage("doc").await;
        let deps = deps_with(Arc::new(CapturingEmail::new()), storage.clone());
        let key = "notations/registry-doc/retainer.pdf";
        let payload = serde_json::to_string(&DocumentPayload::Typst {
            storage_key: key.to_string(),
            typst_source: "Registry body.".into(),
        })
        .unwrap();

        dispatch_step(
            &deps,
            Uuid::from_u128(2),
            &StateName::from("generate_pdf__retainer_pdf"),
            Some(&payload),
        )
        .await
        .expect("generate_pdf dispatch succeeds");

        let stored = storage.get(key).await.expect("PDF persisted");
        assert!(stored.bytes.starts_with(b"%PDF"));
    }

    #[tokio::test]
    async fn non_dispatch_state_is_a_no_op() {
        // A human-driven step (lawyer_review) must not touch any provider
        // and must not require a payload.
        let email = Arc::new(CapturingEmail::new());
        let deps = deps_with(email.clone(), fs_storage("noop").await);
        dispatch_step(
            &deps,
            Uuid::from_u128(3),
            &StateName::from("lawyer_review"),
            None,
        )
        .await
        .expect("no-op dispatch succeeds with no payload");
        assert!(email.captured().is_empty());
    }

    #[tokio::test]
    async fn missing_payload_on_a_dispatched_step_errors() {
        let deps = deps_with(Arc::new(CapturingEmail::new()), fs_storage("missing").await);
        let err = dispatch_step(
            &deps,
            Uuid::from_u128(4),
            &StateName::from("generate_pdf__retainer_pdf"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            StepDispatchError::MissingPayload("generate_pdf")
        ));
    }

    #[tokio::test]
    async fn compliance_step_without_surreal_errors_clearly() {
        // StepDeps::surreal is None here (the email/document tests' default),
        // so a submission step must fail with MissingSurreal rather than panic.
        let deps = deps_with(Arc::new(CapturingEmail::new()), fs_storage("nodb").await);
        let payload = serde_json::to_string(&crate::compliance::CompliancePayload {
            office: "Nevada Secretary of State".into(),
            summary: "Annual report".into(),
            reference: None,
        })
        .unwrap();
        let err = dispatch_step(
            &deps,
            Uuid::from_u128(5),
            &StateName::from("filing__nv_sos"),
            Some(&payload),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            StepDispatchError::MissingSurreal("compliance submission")
        ));
    }
}
