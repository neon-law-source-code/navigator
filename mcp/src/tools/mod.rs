//! Tool registry for the MCP server.
//!
//! Adding a tool is two lines: a `pub mod` here and a `match` arm in
//! [`call_tool`]. Each tool module owns its JSON Schema (returned by
//! `descriptor`) and its handler (`call`).
//!
//! Tool names are namespaced under `aida_` so clients that surface
//! multiple MCP servers (Gemini Enterprise, `LibreChat`) can group
//! Neon Law Navigator's tools cleanly in their UI.

use serde_json::Value;

use store::persons::Role;

use crate::principal::Principal;
use crate::server::McpState;

pub mod aida_bulk_import;
pub mod aida_send_welcome_email;
pub mod aida_spawn_legal_council;
pub mod answer_notation;
pub mod close_project;
pub mod create_notation;
pub mod create_person;
pub mod create_project;
pub mod link_person_project;
pub mod list_deadlines;
pub mod list_entities;
pub mod list_jurisdictions;
pub mod list_projects;
pub mod list_tools;
pub mod project_status;
pub mod show_person;
pub mod validate_notation;

/// Returns the list of tool descriptors `tools/list` advertises.
#[must_use]
pub fn list_tools() -> Vec<Value> {
    vec![
        create_person::descriptor(),
        show_person::descriptor(),
        list_jurisdictions::descriptor(),
        list_entities::descriptor(),
        create_notation::descriptor(),
        answer_notation::descriptor(),
        validate_notation::descriptor(),
        create_project::descriptor(),
        close_project::descriptor(),
        list_deadlines::descriptor(),
        list_projects::descriptor(),
        project_status::descriptor(),
        link_person_project::descriptor(),
        list_tools::descriptor(),
        aida_bulk_import::descriptor(),
        aida_spawn_legal_council::descriptor(),
        aida_send_welcome_email::descriptor(),
    ]
}

/// The tool descriptors a model client is *offered* — every catalog
/// entry that runs without a licensed human approving it first.
///
/// This is deliberately narrower than [`list_tools`], and the gap is not
/// an accident: it is exactly [`requires_confirmation`]. A tool that
/// emails a client, or that creates or answers a Notation, is a
/// supervised act, and neither MCP transport can collect that approval —
/// the protocol has no `input-required` state to pause in. So the act is
/// withheld rather than simulated, and it is performed in `/app`, where
/// a human approves it and the approval is recorded against the matter.
///
/// Filtering rather than only refusing at call time is the point: a tool
/// the model cannot see is a tool it cannot decide to try, so the model
/// is never in the position of proposing an act the transport could not
/// supervise. Both transports still refuse a named-anyway call —
/// [`withheld_message`] is that refusal — because a host may hold a
/// stale catalog.
///
/// Every transport that hands a catalog to a model calls this rather
/// than [`list_tools`]: `mcp::server`'s `tools/list` for the `/mcp`
/// endpoint, and `cli::mcp_bridge` for the stdio bridge. One predicate,
/// two transports, so a newly-gated tool is withheld on both without a
/// second edit. [`list_tools`] itself stays whole — it is the catalog,
/// and `aida_list_tools` and the A2A agent card still describe every
/// capability the firm has.
#[must_use]
pub fn advertised_catalog() -> Vec<Value> {
    list_tools()
        .into_iter()
        .filter(|d| {
            d.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| !requires_confirmation(n))
        })
        .collect()
}

/// Whether `tool_name` (prefixed or unprefixed) is in
/// [`advertised_catalog`]. False for an unknown name as well as a gated
/// one, so a caller that wants to tell those apart checks
/// [`is_known_tool`] too.
#[must_use]
pub fn is_advertised(tool_name: &str) -> bool {
    is_known_tool(tool_name) && !requires_confirmation(tool_name)
}

/// The refusal text for a gated tool a caller named anyway. Shared by
/// both transports so the routing answer a model relays is the same one
/// wherever it dialled in from: this is not "no such tool", it is "not
/// here — do it where the approval can be recorded."
#[must_use]
pub fn withheld_message(tool_name: &str) -> String {
    format!(
        "`{tool_name}` requires a lawyer's explicit approval before it runs, and this \
         connection cannot collect one. Perform it in the Navigator app, where the \
         approval is recorded against the matter."
    )
}

/// Required prefix for every MCP tool name we advertise. Multi-server
/// MCP clients (Gemini Enterprise, `LibreChat`) surface tools from
/// every connected server in one list — namespacing Neon Law Navigator's tools
/// keeps them grouped and avoids name collisions. Enforced by
/// `every_tool_name_starts_with_aida_prefix` in this module's tests.
pub const REQUIRED_PREFIX: &str = "aida_";

/// Tools that only read. These run unconfirmed on every surface.
/// Everything NOT listed here is treated as side-effecting — it writes a
/// row, sends mail, or commits to a matter repo. Defaulting to
/// side-effecting is deliberate: a newly-added tool is treated as a
/// writer until someone consciously marks it read-only here, so we never
/// ship a silent side-effect. Kept in lockstep with [`list_tools`] by
/// `read_only_set_only_names_real_tools`.
///
/// Side-effecting is not the same question as *needs a human to approve
/// it* — that is [`requires_confirmation`], a strictly narrower set.
const READ_ONLY_TOOLS: &[&str] = &[
    "aida_show_person",
    "aida_list_jurisdictions",
    "aida_list_entities",
    "aida_validate_notation",
    "aida_list_deadlines",
    "aida_list_projects",
    "aida_project_status",
    "aida_list_tools",
    "aida_spawn_legal_council",
];

/// Whether a tool mutates state — writes a row, sends an email, commits
/// to a matter repo. Accepts either the prefixed MCP name
/// (`aida_create_person`) or the unprefixed A2A skill id
/// (`create_person`). Tools not listed in [`READ_ONLY_TOOLS`] default to
/// side-effecting, so the safe answer is the default for anything new or
/// unrecognized.
#[must_use]
pub fn is_side_effecting(tool_name: &str) -> bool {
    let prefixed = if tool_name.starts_with(REQUIRED_PREFIX) {
        tool_name.to_string()
    } else {
        format!("{REQUIRED_PREFIX}{tool_name}")
    };
    !READ_ONLY_TOOLS.contains(&prefixed.as_str())
}

/// Side-effecting tools a lawyer principal may run without pausing for
/// an explicit approval.
///
/// Every tool here writes only Navigator's own CRM rows: a contact, an
/// organization, a matter, or the link between them. A lawyer who names
/// one of these has already decided to create the row, the write is
/// visible and correctable in `/app/admin`, and nothing leaves the
/// building. Pausing them would put a round trip in front of every
/// contact in a bulk load without protecting anyone.
///
/// What is deliberately NOT here is the line that matters: a tool that
/// emails a client, or that creates or answers a Notation — a binding
/// legal artifact. Those are supervised acts, so
/// [`requires_confirmation`] pauses them for a licensed human even when
/// a lawyer named the skill directly.
const CONFIRMATION_EXEMPT_TOOLS: &[&str] = &[
    "aida_create_person",
    "aida_create_project",
    "aida_close_project",
    "aida_link_person_project",
    "aida_bulk_import",
];

/// Whether running `tool_name` requires an explicit human approval —
/// the A2A `input-required` pause — rather than running on the caller's
/// lawyer tier alone. Accepts a prefixed MCP name or an unprefixed A2A
/// skill id.
///
/// This is [`is_side_effecting`] minus [`CONFIRMATION_EXEMPT_TOOLS`], and
/// it is fail-closed in the direction that matters: a tool nobody has
/// classified is side-effecting by default and absent from the exempt
/// list, so it requires confirmation. Shipping a new writer that emails
/// a client cannot skip the gate by omission — only by someone adding it
/// to the exempt list on purpose.
#[must_use]
pub fn requires_confirmation(tool_name: &str) -> bool {
    let prefixed = if tool_name.starts_with(REQUIRED_PREFIX) {
        tool_name.to_string()
    } else {
        format!("{REQUIRED_PREFIX}{tool_name}")
    };
    is_side_effecting(&prefixed) && !CONFIRMATION_EXEMPT_TOOLS.contains(&prefixed.as_str())
}

/// Whether `tool_name` (prefixed or unprefixed) names a real tool in the
/// catalog. Callers that gate side-effecting tools use this so an
/// *unknown* skill still falls through to the `Unknown` error rather than
/// being reported as an authorization failure.
#[must_use]
pub fn is_known_tool(tool_name: &str) -> bool {
    let prefixed = if tool_name.starts_with(REQUIRED_PREFIX) {
        tool_name.to_string()
    } else {
        format!("{REQUIRED_PREFIX}{tool_name}")
    };
    list_tools()
        .iter()
        .any(|d| d.get("name").and_then(Value::as_str) == Some(prefixed.as_str()))
}

/// Dispatch a `tools/call`. Returns the MCP `result` payload (the
/// thing that ends up under `Response::result`), or a structured
/// error the dispatcher will repackage as an MCP tool error.
///
/// `principal` is the authenticated email behind the call (populated
/// by an upstream auth layer; see [`crate::Principal`]). Tools that
/// mutate data trust it over any caller-supplied `email`-style
/// argument.
pub async fn call_tool(
    state: &McpState,
    principal: Option<&Principal>,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let surreal = &state.surreal;
    let runtime = state.questionnaire_runtime.as_ref();
    // Per-tool authorization, enforced for EVERY dispatch path (the MCP
    // server, the A2A router loop, and the A2A direct-skill path). A
    // side-effecting tool invoked by an *authenticated* non-lawyer caller
    // is refused here, so authz never depends solely on the endpoint's
    // embedded Rego policy gate or the LLM confirmation flow.
    let caller = resolve_caller(surreal, principal).await?;
    caller.require_tool_authz(name)?;
    // A read is never refused on tier — `require_tool_authz` returns
    // early for anything read-only, and that stays true. What changed is
    // the answer: a read is scoped to the caller's own lens rather than
    // taken over the whole deployment.
    let scope = caller.read_scope();
    match name {
        "aida_create_person" => create_person::call(surreal, arguments).await,
        "aida_show_person" => show_person::call(surreal, &scope, arguments).await,
        "aida_list_jurisdictions" => list_jurisdictions::call(surreal, arguments).await,
        "aida_list_entities" => list_entities::call(surreal, arguments).await,
        "aida_create_notation" => {
            create_notation::call(
                surreal,
                runtime,
                state.storage.as_ref(),
                principal,
                arguments,
            )
            .await
        }
        "aida_answer_notation" => {
            answer_notation::call(surreal, runtime, state.storage.as_ref(), arguments).await
        }
        "aida_validate_notation" => validate_notation::call(arguments).await,
        "aida_create_project" => create_project::call(surreal, principal, arguments).await,
        "aida_close_project" => close_project::call(surreal, arguments).await,
        "aida_list_deadlines" => list_deadlines::call(surreal, &scope, arguments).await,
        "aida_list_projects" => list_projects::call(surreal, &scope, arguments).await,
        "aida_project_status" => project_status::call(surreal, &scope, arguments).await,
        "aida_link_person_project" => link_person_project::call(surreal, arguments).await,
        "aida_bulk_import" => aida_bulk_import::call(surreal, principal, arguments).await,
        "aida_list_tools" => list_tools::call(arguments).await,
        "aida_spawn_legal_council" => aida_spawn_legal_council::call(arguments).await,
        "aida_send_welcome_email" => aida_send_welcome_email::call(state, arguments).await,
        other => Err(ToolError::Unknown(other.to_string())),
    }
}

/// Who a resolved `persons` row says the caller is: which person, and at
/// which tier. The only two facts a dispatch reads off the row, so the
/// row itself is not carried around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    person_id: uuid::Uuid,
    role: Role,
}

/// The acting [`Principal`], resolved once per dispatch.
///
/// Both questions a dispatch asks about its caller — may they run a
/// write, and which matters may they read — are answered from this one
/// lookup rather than one apiece.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Caller {
    /// No [`Principal`] reached the dispatcher. The KIND / local-dev path
    /// where no auth layer ran, and the existing browser harness.
    Anonymous,
    /// A trusted email from the auth layer, and the identity it resolves
    /// to — `None` when there is no `persons` row, because sign-in does
    /// not create a Person.
    ///
    /// The email is the one the caller presented, not the one the row
    /// stores: a refusal should name the address the caller used.
    Authenticated {
        email: String,
        identity: Option<Identity>,
    },
}

/// Resolve the acting principal into the identity both decisions read
/// from.
async fn resolve_caller(
    surreal: &store::surreal::SurrealDb,
    principal: Option<&Principal>,
) -> Result<Caller, ToolError> {
    let Some(email) = principal.map(|p| p.email.trim()).filter(|e| !e.is_empty()) else {
        return Ok(Caller::Anonymous);
    };
    // Case-insensitive, like every other email lookup: a row stored as
    // `Attorney@Example.com` must still answer for a caller whose IdP
    // presents `attorney@example.com`.
    Ok(Caller::Authenticated {
        email: email.to_string(),
        identity: store::persons::find_by_email_ci(surreal, email)
            .await?
            .map(|person| Identity {
                person_id: person.id,
                role: person.role,
            }),
    })
}

impl Caller {
    /// Defense-in-depth tier check for side-effecting tools. An
    /// *authenticated* caller must resolve to a lawyer/admin `persons`
    /// row to run one. An anonymous caller is allowed through: that is
    /// the KIND/local-dev path where no auth layer ran and MCP has no
    /// identity, and in production the OAuth layer always injects a
    /// principal *and* the endpoint is embedded Rego policy-lawyer-gated.
    /// Read-only tools are never gated on tier — they are *scoped*
    /// instead, by [`Self::read_scope`].
    ///
    /// This closes the gap where any allowlisted token was treated as
    /// lawyer: a validated-but-non-lawyer identity (e.g. a Google token
    /// whose email maps to a client) can no longer invoke a write tool.
    fn require_tool_authz(&self, tool_name: &str) -> Result<(), ToolError> {
        if !is_side_effecting(tool_name) {
            return Ok(());
        }
        let Self::Authenticated { email, identity } = self else {
            return Ok(());
        };
        if identity.is_some_and(|i| i.role.is_lawyer_tier()) {
            Ok(())
        } else {
            Err(ToolError::Forbidden(format!(
                "{email} is not lawyer or admin; '{tool_name}' is a privileged operation"
            )))
        }
    }

    /// Which lens this caller's reads answer through.
    fn read_scope(&self) -> ReadScope {
        match self {
            Self::Anonymous => ReadScope::Deployment,
            Self::Authenticated { identity: None, .. } => ReadScope::Unlinked,
            Self::Authenticated {
                identity: Some(identity),
                ..
            } => {
                if identity.role.is_admin_tier() {
                    ReadScope::Directory {
                        role: identity.role,
                    }
                } else {
                    ReadScope::Membership {
                        person_id: identity.person_id,
                        role: identity.role,
                    }
                }
            }
        }
    }
}

/// Which lens a read tool answers through — the store-level identity a
/// [`Principal`]'s email resolves to.
///
/// `Principal` deliberately stays an email: it is the transport's trusted
/// identity, and the Google-OAuth path has nothing else to offer. The
/// person row and tier that participation scoping needs are resolved
/// here instead, once per dispatch, so both protocol surfaces get the
/// same answer and neither middleware has to carry fields it cannot fill.
///
/// These are not degrees of one query. `Membership` reads the
/// participation ledger, `Directory` reads oversight, and `Deployment` is
/// the local path with no identity to read at all — see
/// [`Caller::read_scope`], which is where a caller becomes one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadScope {
    /// No [`Principal`] reached the dispatcher: the KIND / local-dev path
    /// where no auth layer ran, and the existing browser harness. Reads
    /// stay deployment-wide, exactly as they were before this scoping
    /// existed. In production a principal is always injected, so this
    /// variant is not a hole an authenticated caller can fall into.
    Deployment,
    /// Owner or Admin: oversight, not membership. Gets the matter
    /// directory — that a matter exists and who is accountable for it —
    /// and reaches no matter contents. Owner and Admin are not invited to
    /// matters, so they hold no `person_project_roles` row and
    /// [`store::access::visible_projects`] would correctly show them
    /// nothing.
    ///
    /// Carries the caller's own role rather than assuming one: the
    /// directory read re-checks the tier itself, and that check is only
    /// worth anything if it is asked with the tier the caller actually
    /// holds.
    Directory { role: Role },
    /// Lawyer, Clerk, or Client: membership. Scoped by the participation
    /// ledger through [`store::access::visible_projects`], which is the
    /// same predicate `/app/projects` renders from — a lawyer sees the
    /// matters they are on, not all of them.
    Membership { person_id: uuid::Uuid, role: Role },
    /// An authenticated email with no `persons` row. Sees nothing.
    ///
    /// Fail-closed by construction rather than by a check: sign-in does
    /// not create a Person, so an IdP-authenticated stranger is exactly
    /// the caller who must reach no matter at all.
    Unlinked,
}

impl ReadScope {
    /// `true` when this caller reads from the firm's side of the house.
    ///
    /// The firm's people directory is firm-internal: oversight and the
    /// lawyer tier both legitimately search it (a lawyer needs a
    /// `person_id` before they can link one to a matter), while Clerk —
    /// the supervised tier, whose whole surface is one matter's name,
    /// status, and supervising lawyer — and Client do not.
    #[must_use]
    pub fn reads_firm_directory(&self) -> bool {
        match self {
            Self::Deployment | Self::Directory { .. } => true,
            Self::Membership { role, .. } => role.is_lawyer_tier(),
            Self::Unlinked => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    Unknown(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The authenticated principal lacks the tier this tool requires
    /// (e.g. a bulk write reserved for lawyer/admin). The model can't
    /// fix this by retrying with different arguments.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// The write would violate a UNIQUE constraint. Surfaced to the
    /// model as a tool-call failure with `conflict:` so it can correct
    /// the input rather than treat the error as a transient backend
    /// problem to retry. Carries the engine's own message for log
    /// fidelity — it comes from either store.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The store refused a write for a reason the model cannot correct
    /// by retrying with different arguments, and which no module
    /// classified into a narrower variant. Carries the engine's own
    /// message for log fidelity.
    #[error("database error: {0}")]
    Database(String),
    /// Catch-all for internal failures the model can't fix by
    /// retrying with different arguments — workflow-runtime
    /// errors, missing seed data, spec parse failures.
    #[error("internal error: {0}")]
    Internal(String),
}

impl ToolError {
    /// The variant name alone, for logs and spans.
    ///
    /// A `ToolError`'s `Display` text can embed the caller's email or an
    /// argument value (`forbidden: {email} is not lawyer or admin`), and
    /// telemetry carries identifiers and outcomes, never content. Log
    /// sites use this instead of `%err` so a tool failure is still
    /// classifiable without putting a mailbox in the log.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            ToolError::Unknown(_) => "unknown",
            ToolError::InvalidArguments(_) => "invalid_arguments",
            ToolError::NotFound(_) => "not_found",
            ToolError::Forbidden(_) => "forbidden",
            ToolError::Conflict(_) => "conflict",
            ToolError::Database(_) => "database",
            ToolError::Internal(_) => "internal",
        }
    }
}

impl From<store::jurisdictions::JurisdictionError> for ToolError {
    fn from(err: store::jurisdictions::JurisdictionError) -> Self {
        use store::jurisdictions::JurisdictionError as E;
        match err {
            // The unique code index is caller-correctable: the model can
            // retry with a different code.
            E::CodeTaken => ToolError::Conflict(err.to_string()),
            E::Db(_) | E::WriteReturnedNothing => ToolError::Internal(err.to_string()),
        }
    }
}

impl From<store::entities::EntityError> for ToolError {
    fn from(err: store::entities::EntityError) -> Self {
        use store::entities::EntityError as E;
        match err {
            // The firm's own row cannot be forked, and a model that tried
            // can act on that — it is a caller-correctable conflict, not a
            // fault.
            E::FirmAnchorTaken => ToolError::Conflict(err.to_string()),
            E::Db(_) | E::WriteReturnedNothing => ToolError::Internal(err.to_string()),
        }
    }
}

impl From<store::entity_roles::EntityRoleError> for ToolError {
    fn from(err: store::entity_roles::EntityRoleError) -> Self {
        ToolError::Internal(err.to_string())
    }
}

impl From<store::entity_types::EntityTypeError> for ToolError {
    fn from(err: store::entity_types::EntityTypeError) -> Self {
        use store::entity_types::EntityTypeError as E;
        match err {
            // The unique name index is caller-correctable: the model can
            // retry with a different name.
            E::NameTaken => ToolError::Conflict(err.to_string()),
            E::Db(_) | E::WriteReturnedNothing => ToolError::Internal(err.to_string()),
        }
    }
}

impl From<store::persons::PersonError> for ToolError {
    fn from(err: store::persons::PersonError) -> Self {
        use store::persons::PersonError as E;
        match err {
            // The two unique indexes are caller-correctable: the model
            // can retry with a different mailbox or identity.
            E::EmailTaken | E::OidcSubjectTaken => ToolError::Conflict(err.to_string()),
            E::Db(_) | E::WriteReturnedNothing => ToolError::Internal(err.to_string()),
        }
    }
}

impl From<store::people_commands::PeopleCommandError> for ToolError {
    fn from(err: store::people_commands::PeopleCommandError) -> Self {
        use store::people_commands::PeopleCommandError as E;
        match err {
            E::Invalid(m) => ToolError::InvalidArguments(m.to_string()),
            E::EmailConflict => ToolError::Conflict("that email is already in use".into()),
            E::ExternalIdentity => {
                ToolError::Conflict("that Notion user is already linked to another person".into())
            }
            E::NotFound => ToolError::NotFound("person not found".into()),
            E::Blocked(m) => ToolError::Forbidden(m.to_string()),
            E::SendFailed => ToolError::Internal("welcome email send failed".into()),
            E::Db(e) => ToolError::from(e),
        }
    }
}

/// Decode a tool's raw JSON `arguments` into its typed `Args`, mapping
/// any deserialization failure to [`ToolError::InvalidArguments`]. Every
/// tool shares this so the bad-input error convention stays identical
/// across the catalog and each handler reduces to
/// `let args: Args = super::decode_args(arguments)?;`.
pub(crate) fn decode_args<T: serde::de::DeserializeOwned>(
    arguments: &Value,
) -> Result<T, ToolError> {
    serde_json::from_value(arguments.clone())
        .map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        call_tool, list_tools, resolve_caller, Principal, ReadScope, Role, ToolError,
        REQUIRED_PREFIX,
    };
    use crate::server::McpState;
    use serde_json::json;
    use std::sync::Arc;
    use workflows::InMemoryRuntime;

    use store::test_support::mem_surreal;
    async fn state() -> McpState {
        let surreal = mem_surreal().await;
        let runtime: Arc<dyn workflows::StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
        McpState::new(surreal, runtime)
    }

    /// The write-tier check as a dispatch performs it: resolve the
    /// caller once, then ask.
    async fn authz(
        surreal: &store::surreal::SurrealDb,
        principal: Option<&Principal>,
        tool_name: &str,
    ) -> Result<(), ToolError> {
        resolve_caller(surreal, principal)
            .await?
            .require_tool_authz(tool_name)
    }

    /// Generic invariant: every tool descriptor returned by
    /// [`list_tools`] must use the [`REQUIRED_PREFIX`] namespace. This
    /// runs over *whatever* `list_tools` returns, so a future tool
    /// that forgets the prefix fails this test without anyone having
    /// to remember to update the explicit set below.
    #[test]
    fn every_tool_name_starts_with_aida_prefix() {
        let tools = list_tools();
        assert!(
            !tools.is_empty(),
            "list_tools must advertise at least one tool"
        );
        for tool in &tools {
            let name = tool["name"]
                .as_str()
                .unwrap_or_else(|| panic!("tool descriptor has no string `name`: {tool}"));
            assert!(
                name.starts_with(REQUIRED_PREFIX),
                "every tool must be namespaced under `{REQUIRED_PREFIX}`, got `{name}`",
            );
            assert!(
                name.len() > REQUIRED_PREFIX.len(),
                "tool name `{name}` is only the prefix with no suffix",
            );
        }
    }

    /// Explicit registry: the tools we ship today. Pairs with
    /// [`every_tool_name_starts_with_aida_prefix`] — that one enforces
    /// the convention, this one pins the *contents* so a tool can't
    /// be silently removed.
    #[test]
    fn list_tools_advertises_the_expected_registry() {
        let tools = list_tools();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"aida_create_person"));
        assert!(names.contains(&"aida_show_person"));
        assert!(names.contains(&"aida_list_jurisdictions"));
        assert!(names.contains(&"aida_list_entities"));
        assert!(names.contains(&"aida_create_notation"));
        assert!(names.contains(&"aida_answer_notation"));
        assert!(names.contains(&"aida_validate_notation"));
        assert!(names.contains(&"aida_create_project"));
        assert!(names.contains(&"aida_close_project"));
        assert!(names.contains(&"aida_list_deadlines"));
        assert!(names.contains(&"aida_list_projects"));
        assert!(names.contains(&"aida_project_status"));
        assert!(names.contains(&"aida_link_person_project"));
        assert!(names.contains(&"aida_bulk_import"));
        assert!(names.contains(&"aida_list_tools"));
        assert!(names.contains(&"aida_spawn_legal_council"));
        assert!(names.contains(&"aida_send_welcome_email"));
    }

    #[test]
    fn read_only_tools_are_not_side_effecting() {
        // The read-only allowlist must classify as no-confirmation, by
        // both their prefixed MCP name and unprefixed A2A skill id.
        for name in super::READ_ONLY_TOOLS {
            assert!(
                !super::is_side_effecting(name),
                "`{name}` is on the read-only allowlist but classified side-effecting"
            );
            let unprefixed = name.strip_prefix(REQUIRED_PREFIX).unwrap();
            assert!(
                !super::is_side_effecting(unprefixed),
                "`{unprefixed}` (unprefixed) should match the read-only allowlist"
            );
        }
    }

    #[test]
    fn writers_are_side_effecting_and_default_is_safe() {
        // Known writers must be gated...
        for name in [
            "aida_create_person",
            "aida_send_welcome_email",
            "aida_create_project",
            "aida_close_project",
            "aida_create_notation",
            "aida_bulk_import",
        ] {
            assert!(super::is_side_effecting(name), "`{name}` must be gated");
        }
        // ...and unprefixed forms classify the same.
        assert!(super::is_side_effecting("create_person"));
        assert!(super::is_side_effecting("send_welcome_email"));
        // An unknown tool defaults to side-effecting — the safe default.
        assert!(super::is_side_effecting("aida_some_future_writer"));
        assert!(super::is_side_effecting("totally_unknown"));
    }

    #[test]
    fn advertised_catalog_withholds_exactly_the_confirmation_gated_tools() {
        let catalog = super::advertised_catalog();
        let advertised: Vec<&str> = catalog.iter().filter_map(|t| t["name"].as_str()).collect();

        for gated in [
            "aida_create_notation",
            "aida_answer_notation",
            "aida_send_welcome_email",
        ] {
            assert!(
                super::requires_confirmation(gated),
                "`{gated}` is expected to be a supervised act"
            );
            assert!(
                !advertised.contains(&gated),
                "`{gated}` must not be advertised; got {advertised:?}"
            );
        }

        // Every other catalog entry is still offered — the filter withholds
        // the supervised acts and nothing else.
        let tools = list_tools();
        let whole: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for name in &whole {
            assert_eq!(
                advertised.contains(name),
                !super::requires_confirmation(name),
                "`{name}` advertised state disagrees with requires_confirmation"
            );
        }
        assert_eq!(advertised.len(), whole.len() - 3);
    }

    #[test]
    fn is_advertised_separates_gated_from_unknown() {
        // Both answer `false`, and the distinction that matters to a caller
        // is `is_known_tool` — a gated tool is real and routed elsewhere, an
        // unknown one is a mistake.
        assert!(super::is_advertised("aida_create_person"));
        // The unprefixed A2A skill id resolves the same way.
        assert!(super::is_advertised("create_person"));

        assert!(!super::is_advertised("aida_create_notation"));
        assert!(super::is_known_tool("aida_create_notation"));

        assert!(!super::is_advertised("aida_not_a_tool"));
        assert!(!super::is_known_tool("aida_not_a_tool"));
    }

    #[test]
    fn withheld_message_names_the_tool_and_where_to_perform_it() {
        let text = super::withheld_message("aida_send_welcome_email");
        assert!(text.contains("aida_send_welcome_email"), "got `{text}`");
        assert!(text.contains("Navigator app"), "got `{text}`");
        assert!(text.contains("recorded against the matter"), "got `{text}`");
    }

    #[test]
    fn read_only_set_only_names_real_tools() {
        // Guard against the allowlist drifting from the catalog: every
        // entry must be a tool we actually advertise.
        let tools = list_tools();
        let real: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for name in super::READ_ONLY_TOOLS {
            assert!(
                real.contains(name),
                "READ_ONLY_TOOLS lists `{name}`, which is not in list_tools()"
            );
        }
    }

    #[test]
    fn confirmation_exempt_set_only_names_real_side_effecting_tools() {
        // Same drift guard as the read-only allowlist, plus the stronger
        // claim: exempting a read-only tool from confirmation is
        // meaningless, so every entry must actually be a writer.
        let tools = list_tools();
        let real: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for name in super::CONFIRMATION_EXEMPT_TOOLS {
            assert!(
                real.contains(name),
                "CONFIRMATION_EXEMPT_TOOLS lists `{name}`, which is not in list_tools()"
            );
            assert!(
                super::is_side_effecting(name),
                "CONFIRMATION_EXEMPT_TOOLS lists read-only `{name}`; only writers need exempting"
            );
        }
    }

    #[test]
    fn requires_confirmation_gates_client_facing_and_notation_writes() {
        // The three that reach a client or move a binding artifact.
        for name in [
            "aida_send_welcome_email",
            "aida_create_notation",
            "aida_answer_notation",
        ] {
            assert!(
                super::requires_confirmation(name),
                "`{name}` reaches a client or a binding artifact and must be confirmed"
            );
        }
        // The CRM writers a lawyer may run straight through, so a bulk
        // contact load is not one round trip per row.
        for name in [
            "aida_create_person",
            "aida_create_project",
            "aida_close_project",
            "aida_link_person_project",
            "aida_bulk_import",
        ] {
            assert!(
                !super::requires_confirmation(name),
                "`{name}` writes only Navigator's own CRM rows and must not pause"
            );
        }
    }

    #[test]
    fn requires_confirmation_never_gates_a_read() {
        for name in super::READ_ONLY_TOOLS {
            assert!(
                !super::requires_confirmation(name),
                "read-only `{name}` must never require confirmation"
            );
        }
    }

    #[test]
    fn requires_confirmation_accepts_prefixed_and_unprefixed_names() {
        // A2A carries the unprefixed skill id; MCP carries the prefix.
        // Both must reach the same verdict or the gate depends on which
        // protocol asked.
        assert!(super::requires_confirmation("send_welcome_email"));
        assert!(super::requires_confirmation("aida_send_welcome_email"));
        assert!(!super::requires_confirmation("create_person"));
        assert!(!super::requires_confirmation("aida_create_person"));
    }

    #[test]
    fn requires_confirmation_is_fail_closed_for_an_unclassified_tool() {
        // A writer nobody has classified is absent from both lists, so
        // it must land on the gated side. Shipping a new client-facing
        // tool cannot skip the pause by omission.
        assert!(super::requires_confirmation("aida_some_future_writer"));
        assert!(super::requires_confirmation("totally_unknown"));
    }

    #[tokio::test]
    async fn call_tool_with_unknown_name_returns_unknown_error() {
        let s = state().await;
        let err = call_tool(&s, None, "does_not_exist", &json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Unknown(name) if name == "does_not_exist"));
    }

    #[tokio::test]
    async fn require_tool_authz_blocks_non_lawyer_yet_allows_anonymous_and_read_only() {
        let s = state().await;
        store::persons::create(
            &s.surreal,
            &store::persons::NewPerson::with_role(
                "Client",
                "client@example.com",
                store::persons::Role::Client,
            ),
        )
        .await
        .unwrap();
        let client = Principal::new("client@example.com");

        // Anonymous (dev / no auth layer) is allowed even for writes.
        assert!(authz(&s.surreal, None, "aida_create_project").await.is_ok());
        // A read-only tool is never gated on tier.
        assert!(authz(&s.surreal, Some(&client), "aida_show_person")
            .await
            .is_ok());
        // A side-effecting tool by an authenticated client-tier caller is
        // refused — the core of the fix.
        assert!(matches!(
            authz(&s.surreal, Some(&client), "aida_create_project").await,
            Err(ToolError::Forbidden(_))
        ));
        // An authenticated caller with no `persons` row is also refused.
        let ghost = Principal::new("ghost@example.com");
        assert!(matches!(
            authz(&s.surreal, Some(&ghost), "aida_create_project").await,
            Err(ToolError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn require_tool_authz_matches_lawyer_email_case_insensitively() {
        // A lawyer row stored with mixed-case email. The gate matches the
        // stored `email_lower` field, so a differently-cased principal is
        // authorized instead of being rejected before the tool's own
        // resolver runs.
        let s = state().await;
        store::persons::create(
            &s.surreal,
            &store::persons::NewPerson::with_role(
                "Attorney",
                "Attorney@Example.com",
                store::persons::Role::Lawyer,
            ),
        )
        .await
        .unwrap();

        let caller = Principal::new("attorney@example.com");
        assert!(
            authz(&s.surreal, Some(&caller), "aida_create_project")
                .await
                .is_ok(),
            "a mixed-case lawyer row must authorize a lower-case caller through the dispatched gate"
        );
    }

    #[tokio::test]
    async fn call_tool_dispatches_aida_validate_notation() {
        let s = state().await;
        let result = call_tool(
            &s,
            None,
            "aida_validate_notation",
            &json!({ "contents": "# H\n", "markdown_only": true }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["clean"], true);
    }

    // -----------------------------------------------------------------
    // Read scope. The tier check above refuses a *write*; these decide
    // which lens a *read* answers through.
    // -----------------------------------------------------------------

    async fn scope_for(
        surreal: &store::surreal::SurrealDb,
        principal: Option<&Principal>,
    ) -> ReadScope {
        resolve_caller(surreal, principal)
            .await
            .unwrap()
            .read_scope()
    }

    async fn seed_at(
        surreal: &store::surreal::SurrealDb,
        email: &str,
        role: Role,
    ) -> store::persons::Person {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(email, email, role),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn no_principal_resolves_to_the_deployment_scope() {
        // The KIND / local-dev path and the browser harness: no auth
        // layer ran, so reads stay deployment-wide as they always were.
        let surreal = mem_surreal().await;
        assert_eq!(scope_for(&surreal, None).await, ReadScope::Deployment);
        // An empty or whitespace email is not an identity either.
        let blank = Principal::new("   ");
        assert_eq!(
            scope_for(&surreal, Some(&blank)).await,
            ReadScope::Deployment
        );
    }

    #[tokio::test]
    async fn owner_and_admin_resolve_to_the_directory_lens() {
        for role in [Role::Owner, Role::Admin] {
            let surreal = mem_surreal().await;
            seed_at(&surreal, "boss@example.com", role).await;
            assert_eq!(
                scope_for(&surreal, Some(&Principal::new("boss@example.com"))).await,
                ReadScope::Directory { role },
                "{role:?} is oversight, not membership"
            );
        }
    }

    #[tokio::test]
    async fn the_membership_tiers_resolve_to_their_own_person_and_role() {
        for role in [Role::Lawyer, Role::Clerk, Role::Client] {
            let surreal = mem_surreal().await;
            let person = seed_at(&surreal, "member@example.com", role).await;
            assert_eq!(
                scope_for(&surreal, Some(&Principal::new("member@example.com"))).await,
                ReadScope::Membership {
                    person_id: person.id,
                    role
                },
                "{role:?} reads through the participation ledger"
            );
        }
    }

    #[tokio::test]
    async fn the_lookup_is_case_insensitive_like_every_other_email_lookup() {
        // A row stored mixed-case must still scope a caller whose IdP
        // presents the address lowercased.
        let surreal = mem_surreal().await;
        let person = seed_at(&surreal, "Attorney@Example.com", Role::Lawyer).await;
        assert_eq!(
            scope_for(&surreal, Some(&Principal::new("attorney@example.com"))).await,
            ReadScope::Membership {
                person_id: person.id,
                role: Role::Lawyer
            }
        );
    }

    #[tokio::test]
    async fn an_authenticated_email_with_no_persons_row_is_unlinked() {
        // Sign-in does not create a Person, so an IdP-authenticated
        // stranger is exactly the caller who must reach nothing.
        let surreal = mem_surreal().await;
        assert_eq!(
            scope_for(&surreal, Some(&Principal::new("stranger@example.com"))).await,
            ReadScope::Unlinked
        );
    }

    #[test]
    fn only_the_firm_side_reads_the_people_directory() {
        assert!(ReadScope::Deployment.reads_firm_directory());
        assert!(ReadScope::Directory { role: Role::Owner }.reads_firm_directory());
        assert!(ReadScope::Directory { role: Role::Admin }.reads_firm_directory());
        assert!(!ReadScope::Unlinked.reads_firm_directory());
        for (role, expected) in [
            (Role::Lawyer, true),
            // Clerk is the supervised tier, not a narrower Lawyer: its
            // surface is one matter's name, status, and supervising
            // lawyer, which a firm-wide people search is not.
            (Role::Clerk, false),
            (Role::Client, false),
        ] {
            assert_eq!(
                ReadScope::Membership {
                    person_id: uuid::Uuid::now_v7(),
                    role
                }
                .reads_firm_directory(),
                expected,
                "{role:?}"
            );
        }
    }

    /// A read is still never refused on *tier* — `require_tool_authz`
    /// returns early for anything read-only, and threading a scope
    /// through did not change that. What changed is the answer.
    #[tokio::test]
    async fn a_read_tool_is_not_gated_by_the_write_tier_check() {
        let state = state().await;
        store::persons::create(
            &state.surreal,
            &store::persons::NewPerson::with_role(
                "client@example.com",
                "client@example.com",
                Role::Client,
            ),
        )
        .await
        .unwrap();
        let principal = Principal::new("client@example.com");
        // A client cannot run a write tool...
        let write = call_tool(
            &state,
            Some(&principal),
            "aida_create_project",
            &json!({ "name": "Nope" }),
        )
        .await;
        assert!(matches!(write, Err(ToolError::Forbidden(_))));
        // ...but the read runs, and answers with their own empty list
        // rather than a refusal.
        let read = call_tool(&state, Some(&principal), "aida_list_projects", &json!({}))
            .await
            .unwrap();
        assert_eq!(read["structuredContent"]["count"], 0);
        assert_eq!(read["structuredContent"]["lens"], "membership");
    }
}
