//! Typed error returned by every step of the `devx gcp setup`
//! pipeline. Replaces the bare `anyhow::Error` that the gcp/* modules
//! used to surface; the binary's `main.rs` still uses `anyhow` at the
//! outermost layer and `?` converts `SetupError` into `anyhow::Error`
//! via the blanket `From<E: std::error::Error>` impl.

use std::time::Duration;

use thiserror::Error;

use super::client::ClientError;

/// Anything the setup pipeline can fail with.
#[derive(Debug, Error)]
pub enum SetupError {
    /// A required deployment setting was not supplied to the setup command.
    #[error("missing required deployment setting: {0}")]
    MissingConfiguration(&'static str),

    /// The deployment's public base URL cannot safely become a CORS origin.
    #[error("invalid NAV_BASE_URL for assets CORS: {0}")]
    InvalidPublicBaseUrl(String),

    /// A project ID was offered for a provisioning target that a different
    /// target already owns. Project IDs are immutable in GCP and no project
    /// serves two tenants; see `docs/environments.md`.
    #[error(
        "project `{project_id}` is recorded as {recorded}, so it cannot be provisioned as \
         {requested}"
    )]
    TenantConflict {
        project_id: String,
        recorded: &'static str,
        requested: &'static str,
    },

    /// An organization policy refused an IAM write. After the org split the
    /// environment service accounts are foreign identities to a cross-org
    /// registry, so every `setIamPolicy` — including a routine provisioner
    /// re-run — is evaluated against domain restricted sharing. Name the
    /// constraint and the principal rather than surfacing a bare 403.
    #[error(
        "organization policy `{constraint}` refused `{principal}` on {resource}: that principal's \
         Cloud Identity customer is not admitted by domain restricted sharing. Allow the customer \
         on the registry's organization, or scope an exception on the registry project, then \
         re-run. GCP said: {body}"
    )]
    OrgPolicyRefused {
        constraint: &'static str,
        principal: String,
        resource: String,
        body: String,
    },

    /// The HTTP/auth layer in [`super::client::GcpClient`] surfaced
    /// an error (network, non-2xx the client itself wanted to flag,
    /// auth token acquisition).
    #[error(transparent)]
    Client(#[from] ClientError),

    /// A GCP REST call returned a non-2xx status. `operation` is a
    /// human-readable summary (`"create bucket navigator-prod-assets"`,
    /// `"batchEnable"`, `"create SQL instance"`) and `body` carries
    /// whatever GCP wrote into the response. The numeric status code
    /// stays in the message verbatim so existing tests that grep
    /// `format!("{err}")` for `"403"` / `"409"` keep matching.
    #[error("{operation} failed with status {status}: {body}")]
    BadStatus {
        operation: String,
        status: u16,
        body: String,
    },

    /// JSON parsing failed — usually a response body that we expect
    /// to be a well-formed `Operation` resource.
    #[error("parse {what}: {source}")]
    Json {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// A long-running operation reported its own error. Carries the
    /// raw JSON `error` object as a string so log scrapers and the
    /// `permission denied` test assertion can still match.
    #[error("operation failed: {0}")]
    OperationFailed(String),

    /// Polling for a long-running operation exceeded its budget.
    #[error("operation {name} did not complete within {timeout:?}")]
    OperationTimeout { name: String, timeout: Duration },

    /// A GCP response was structurally invalid — e.g. an `Operation`
    /// resource missing its `name` field.
    #[error("malformed GCP response: {0}")]
    Malformed(&'static str),

    /// A Project code cannot become a publisher service-account id.
    ///
    /// Either it is not a well-formed Project code, or it is too long: an
    /// account id is capped at 30 characters and the code travels verbatim,
    /// because a shortened code collides silently with every other code sharing
    /// its prefix. Raised before any call is made, so nothing is provisioned.
    #[error("Project code `{code}` cannot own a publisher service account: {reason}")]
    PublisherCodeRefused { code: String, reason: String },

    /// Live state and the requested state are both plausible, and the
    /// provisioner cannot tell which is intended — so it refuses instead of
    /// overwriting. Distinct from [`Self::BadStatus`]: no call failed, and
    /// distinct from an idempotent no-op: converging would destroy something.
    #[error("{operation} refused: {detail}")]
    AmbiguousLiveState { operation: String, detail: String },

    /// A shell-out (gcloud, kubectl) returned non-zero AND the stderr
    /// did not match the "already exists" idempotency pattern. The
    /// numeric exit code stays in the message verbatim so log
    /// scrapers can grep for it.
    #[error("{operation} failed: `{command}` exited {exit}: {stderr}")]
    ShellFailed {
        operation: &'static str,
        command: String,
        exit: i32,
        stderr: String,
    },
}

/// Convenience alias used throughout `devx::gcp`.
pub type SetupResult<T> = Result<T, SetupError>;
