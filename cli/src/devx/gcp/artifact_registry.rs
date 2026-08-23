//! Provision the **private** Artifact Registry that Navigator's
//! container images live in, plus the CI identity that pushes to it.
//!
//! CI (`deploy.yml`) builds every navigator image and pushes it to
//! `<region>-docker.pkg.dev/<project>/<repo>/<image>:<tag>`. The repo
//! is private — only principals inside the firm's GCP project may
//! pull — so this module wires:
//!
//! 1. [`ensure_repository`] — one Docker-format Artifact Registry
//!    repository (default `navigator`) that holds every image path.
//! 2. [`ensure_cleanup_policy`] — keep the last [`RETAINED_VERSIONS`]
//!    versions of each image and delete the rest, so storage is capped
//!    without retention depending on how often anyone releases.
//! 3. [`ensure_ci_service_account`] + a repo-scoped
//!    `roles/artifactregistry.writer` binding — the identity CI pushes
//!    as.
//! 4. A repo-scoped `roles/artifactregistry.reader` binding for the
//!    GKE Autopilot node service account so prod can pull.
//! 5. A Workload Identity Federation pool + provider so GitHub Actions
//!    authenticates **keyless** (no downloaded SA key), restricted to
//!    this one repository.
//!
//! ## Idempotency
//!
//! Every `ensure_*` follows the pipeline convention: `create` calls
//! POST unconditionally and treat HTTP **409 Conflict** as success;
//! IAM bindings read the live policy and skip the write when it already
//! says what it should; the cleanup policy is a PATCH, which is safe to
//! re-apply. A re-run after a partial failure converges.
//!
//! Two bindings differ in what "should" means. The registry repository is
//! additive — it carries one reader per runtime project alongside the
//! pusher's writer. The pusher's own `workloadIdentityUser` binding is
//! exclusive: exactly one repository may impersonate it, so a re-run
//! revokes a principal left behind by an org rename. An additive ensure
//! cannot revoke, and a provisioner that only adds never will.

use serde_json::{json, Value};

use super::client::{GcpClient, GcpService, Mode};
use super::error::{SetupError, SetupResult};
use super::{iap, lro, tenants, SetupConfig};

/// Cleanup-policy retention: how many versions of each image to keep.
///
/// A COUNT rather than an age, and that is the whole point. An age-based
/// rule only bounds storage safely while releases are frequent enough to
/// outrun it: under the nightly train every running tag was a day old, so a
/// 7-day window could never reach one. Releases are tag-driven now (see
/// `docs/gitops.md`), and a quiet fortnight under an age rule would have let
/// Artifact Registry delete the exact versions production was running — pods
/// keep serving what they already pulled, but a reschedule cannot pull and
/// `ops ship` refuses a tag the registry no longer holds. A count cannot
/// expire: the last [`RETAINED_VERSIONS`] releases stay pullable however long
/// the gap between them.
pub const RETAINED_VERSIONS: u32 = 10;
/// Keep the most recent [`RETAINED_VERSIONS`] versions of every image.
pub const KEEP_POLICY_ID: &str = "keep-last-10-versions";
/// Delete every version the keep policy above does not retain.
pub const DELETE_POLICY_ID: &str = "delete-unretained-versions";

/// Workload Identity Federation pool + provider ids for GitHub Actions.
pub const WIF_POOL_ID: &str = "github";
pub const WIF_PROVIDER_ID: &str = "github-oidc";
/// The Actions OIDC token issuer for github.com, which is the host Navigator's
/// repositories live on.
///
/// The wrong value here is silent until the first publish: a pool that trusts an
/// issuer the tokens are not minted by accepts the create, reports healthy, and
/// then fails every token exchange. That is the failure this constant exists to
/// keep single-sourced, and it runs in both directions — a self-hoster on their
/// own tenant mints from that tenant's own subdomain and must not copy this
/// value. Read it off the host rather than copying it:
///
/// ```text
/// curl https://token.actions.githubusercontent.com/.well-known/openid-configuration
/// ```
pub const GITHUB_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

pub(super) const WRITER_ROLE: &str = "roles/artifactregistry.writer";
pub(super) const READER_ROLE: &str = "roles/artifactregistry.reader";
const WORKLOAD_IDENTITY_USER_ROLE: &str = "roles/iam.workloadIdentityUser";

/// The organization-policy constraint that gates domain restricted sharing.
/// Once the Foundation's project sits in its own organization, `neon-law` and
/// `neon-law-org` service accounts are foreign identities to it, so every
/// `setIamPolicy` against the hub repository is evaluated against this
/// constraint — including a routine provisioner re-run.
pub const DOMAIN_RESTRICTED_SHARING_CONSTRAINT: &str = "constraints/iam.allowedPolicyMemberDomains";

/// Outcome of an idempotent create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureOutcome {
    Created,
    AlreadyExists,
}

/// The CI pusher service account email for `account_id` in `project_id`.
#[must_use]
pub fn ci_service_account_email(account_id: &str, project_id: &str) -> String {
    format!("{account_id}@{project_id}.iam.gserviceaccount.com")
}

/// The Workload Identity principal set that maps the GitHub repo's
/// Actions runs onto the CI service account.
#[must_use]
pub fn wif_principal_set(project_number: &str, github_repo: &str) -> String {
    format!(
        "principalSet://iam.googleapis.com/projects/{project_number}/locations/global/\
         workloadIdentityPools/{WIF_POOL_ID}/attribute.repository/{github_repo}"
    )
}

/// Wire `project_id` to the container images it runs.
///
/// Two shapes, chosen by `config.images_project_id`:
///
/// - **Hub-and-spoke** (`--images-project-id ghcr`): the registry,
///   the CI pusher, and the WIF pool live in the hub, provisioned once by
///   `ops gcp hub setup`. This environment only needs pull rights on the hub
///   repository, so [`ensure_cross_project_reader`] is the whole story.
/// - **Single project** (the flag omitted): registry, CI identity, and puller
///   all live in `project_id`, as they do for a fork that runs one project.
pub async fn ensure(client: &GcpClient, project_id: &str, config: &SetupConfig) -> SetupResult<()> {
    match config.images_project_id.as_deref() {
        Some(images_project_id) => {
            ensure_cross_project_reader(client, project_id, images_project_id, config).await?;
            Ok(())
        }
        None => ensure_in_project(client, project_id, config).await,
    }
}

/// Grant `environment_project_id`'s workload puller
/// `roles/artifactregistry.reader` on the **hub's** repository.
///
/// The binding is written in the hub project, so it is the call that domain
/// restricted sharing evaluates after the org split; [`ensure_iam_member`]
/// turns a refusal into [`SetupError::OrgPolicyRefused`] rather than a bare
/// 403.
pub async fn ensure_cross_project_reader(
    client: &GcpClient,
    environment_project_id: &str,
    images_project_id: &str,
    config: &SetupConfig,
) -> SetupResult<BindingOutcome> {
    tenants::validate_images_project(environment_project_id, images_project_id)?;
    let project_number = project_number(client, environment_project_id).await?;
    let puller = format!("serviceAccount:{project_number}-compute@developer.gserviceaccount.com");
    ensure_repo_iam_member(
        client,
        images_project_id,
        &config.region,
        &config.artifact_registry_repo,
        READER_ROLE,
        &puller,
    )
    .await
}

/// Provision the full private-registry story inside `project_id`. See the
/// module docs for the order; each step is idempotent.
async fn ensure_in_project(
    client: &GcpClient,
    project_id: &str,
    config: &SetupConfig,
) -> SetupResult<()> {
    let location = &config.region;
    let repo = &config.artifact_registry_repo;

    ensure_repository(client, project_id, location, repo).await?;
    ensure_cleanup_policy(client, project_id, location, repo).await?;

    let ci_sa = ci_service_account_email(&config.ci_pusher_account_id, project_id);
    ensure_ci_service_account(client, project_id, &config.ci_pusher_account_id).await?;
    ensure_repo_iam_member(
        client,
        project_id,
        location,
        repo,
        WRITER_ROLE,
        &format!("serviceAccount:{ci_sa}"),
    )
    .await?;

    let project_number = project_number(client, project_id).await?;
    let gke_puller =
        format!("serviceAccount:{project_number}-compute@developer.gserviceaccount.com");
    ensure_repo_iam_member(client, project_id, location, repo, READER_ROLE, &gke_puller).await?;

    ensure_wif_pool(client, project_id).await?;
    ensure_wif_provider(client, project_id, &config.github_repo).await?;
    ensure_wif_impersonation(
        client,
        project_id,
        &ci_sa,
        &wif_principal_set(&project_number, &config.github_repo),
    )
    .await?;
    Ok(())
}

/// Idempotently create the Docker-format repository. LRO on success;
/// 409 means it already exists.
pub async fn ensure_repository(
    client: &GcpClient,
    project_id: &str,
    location: &str,
    repo: &str,
) -> SetupResult<EnsureOutcome> {
    let path =
        format!("/v1/projects/{project_id}/locations/{location}/repositories?repositoryId={repo}");
    let body = json!({
        "format": "DOCKER",
        "description": "Navigator container images (private, last 10 versions per image)",
    });
    let resp = client
        .post_json(GcpService::ArtifactRegistry, &path, &body)
        .await?;
    match resp.status_u16() {
        200..=299 => {
            let op: Value =
                serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
                    what: "create repository operation",
                    source,
                })?;
            lro::wait(client, GcpService::ArtifactRegistry, &op, "/v1/{name}").await?;
            Ok(EnsureOutcome::Created)
        }
        409 => Ok(EnsureOutcome::AlreadyExists),
        other => Err(SetupError::BadStatus {
            operation: format!("create artifact registry repository {repo}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// PATCH the repository's `cleanupPolicies` to the count-based pair: keep the
/// last [`RETAINED_VERSIONS`] versions of each image, delete the rest.
///
/// Idempotent by construction — re-applying the same policies is a no-op on
/// Google's side. `cleanupPolicies` is a map named in `updateMask`, so this
/// request REPLACES the whole set rather than merging into it; that is what
/// retires the old age-based `delete-older-than-7d` rule on the next run
/// instead of leaving it in place to delete a version the keep policy meant to
/// retain.
///
/// The two policies are one unit. `KEEP` alone deletes nothing, and the
/// `DELETE` half matches EVERY version (`tagState: ANY`) — applied by itself it
/// would empty the repository. Keep policies take precedence over delete
/// policies in Artifact Registry, and that precedence is the only thing making
/// the pair mean "keep ten, delete the rest".
///
/// `keepCount` is per package, so each image keeps its own last ten. A release
/// pushes one version per image under two tags (`YY.M.D` and `latest`) — one
/// digest, one version — so ten versions is ten releases, and with one tag per
/// calendar day it is also at least the last ten release days.
pub async fn ensure_cleanup_policy(
    client: &GcpClient,
    project_id: &str,
    location: &str,
    repo: &str,
) -> SetupResult<()> {
    let path = format!(
        "/v1/projects/{project_id}/locations/{location}/repositories/{repo}?updateMask=cleanupPolicies"
    );
    let body = json!({
        "cleanupPolicies": {
            KEEP_POLICY_ID: {
                "id": KEEP_POLICY_ID,
                "action": "KEEP",
                "mostRecentVersions": { "keepCount": RETAINED_VERSIONS }
            },
            DELETE_POLICY_ID: {
                "id": DELETE_POLICY_ID,
                "action": "DELETE",
                "condition": { "tagState": "ANY" }
            }
        }
    });
    let resp = client
        .patch_json(GcpService::ArtifactRegistry, &path, &body)
        .await?;
    match resp.status_u16() {
        200..=299 => Ok(()),
        other => Err(SetupError::BadStatus {
            operation: format!("set cleanup policy on repository {repo}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Idempotently create the CI pusher service account. 409 = exists.
pub async fn ensure_ci_service_account(
    client: &GcpClient,
    project_id: &str,
    account_id: &str,
) -> SetupResult<EnsureOutcome> {
    let path = format!("/v1/projects/{project_id}/serviceAccounts");
    let body = json!({
        "accountId": account_id,
        "serviceAccount": { "displayName": "Navigator CI image pusher" }
    });
    let resp = client.post_json(GcpService::Iam, &path, &body).await?;
    match resp.status_u16() {
        200..=299 => Ok(EnsureOutcome::Created),
        409 => Ok(EnsureOutcome::AlreadyExists),
        other => Err(SetupError::BadStatus {
            operation: format!("create service account {account_id}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Idempotently add `member` to `role` on the repository's IAM policy.
/// Reads the policy first and skips the write when nothing changes.
pub async fn ensure_repo_iam_member(
    client: &GcpClient,
    project_id: &str,
    location: &str,
    repo: &str,
    role: &str,
    member: &str,
) -> SetupResult<BindingOutcome> {
    let resource = format!("/v1/projects/{project_id}/locations/{location}/repositories/{repo}");
    ensure_iam_member(
        client,
        GcpService::ArtifactRegistry,
        &resource,
        role,
        member,
        PolicyRead::Get,
        // Additive: the hub repository legitimately carries several readers
        // (one per runtime project) alongside the pusher's writer binding.
        MemberMode::Additive,
    )
    .await
}

/// Idempotently create the GitHub Actions Workload Identity pool.
pub async fn ensure_wif_pool(client: &GcpClient, project_id: &str) -> SetupResult<EnsureOutcome> {
    let path = format!(
        "/v1/projects/{project_id}/locations/global/workloadIdentityPools?workloadIdentityPoolId={WIF_POOL_ID}"
    );
    let body = json!({ "displayName": "GitHub Actions" });
    create_lro_or_conflict(client, GcpService::Iam, &path, &body, "create WIF pool").await
}

/// The refs a publish may run from: `main`, plus the release tags `ops ship`
/// rolls. Anything else fails token exchange rather than the push.
const PUBLISHING_REFS: &str =
    "(assertion.ref == 'refs/heads/main' || assertion.ref.startsWith('refs/tags/'))";

/// The attribute condition guarding token exchange.
///
/// Pinned to the full `owner/repo`, not just the owner: a fork carries its own
/// `repository` claim, so it is refused here instead of reaching the registry.
#[must_use]
pub fn wif_attribute_condition(github_repo: &str) -> String {
    format!("assertion.repository == '{github_repo}' && {PUBLISHING_REFS}")
}

fn wif_provider_body(github_repo: &str) -> Value {
    json!({
        "displayName": "GitHub OIDC",
        "oidc": { "issuerUri": GITHUB_OIDC_ISSUER },
        "attributeMapping": {
            "google.subject": "assertion.sub",
            "attribute.repository": "assertion.repository",
            "attribute.repository_owner": "assertion.repository_owner"
        },
        "attributeCondition": wif_attribute_condition(github_repo)
    })
}

/// Idempotently create the GitHub OIDC provider under the pool, converging an
/// existing one onto the current issuer, mapping, and condition.
///
/// The convergence half is load-bearing, not defensive. A provider built
/// against the wrong issuer answers the create with 409, so a create-only
/// `ensure` reports `AlreadyExists` over a resource that can never mint a
/// token — the command claims success and changes nothing.
pub async fn ensure_wif_provider(
    client: &GcpClient,
    project_id: &str,
    github_repo: &str,
) -> SetupResult<EnsureOutcome> {
    let collection = format!(
        "/v1/projects/{project_id}/locations/global/workloadIdentityPools/{WIF_POOL_ID}/providers"
    );
    let body = wif_provider_body(github_repo);
    let outcome = create_lro_or_conflict(
        client,
        GcpService::Iam,
        &format!("{collection}?workloadIdentityPoolProviderId={WIF_PROVIDER_ID}"),
        &body,
        "create WIF provider",
    )
    .await?;
    if outcome == EnsureOutcome::AlreadyExists {
        patch_lro(
            client,
            GcpService::Iam,
            &format!(
                "{collection}/{WIF_PROVIDER_ID}\
                 ?updateMask=oidc,attributeMapping,attributeCondition"
            ),
            &body,
            "update WIF provider",
        )
        .await?;
    }
    Ok(outcome)
}

/// Idempotently let the federated GitHub principal impersonate the CI
/// service account (`roles/iam.workloadIdentityUser`).
pub async fn ensure_wif_impersonation(
    client: &GcpClient,
    project_id: &str,
    ci_service_account: &str,
    principal_set: &str,
) -> SetupResult<BindingOutcome> {
    let resource = format!("/v1/projects/{project_id}/serviceAccounts/{ci_service_account}");
    ensure_iam_member(
        client,
        GcpService::Iam,
        &resource,
        WORKLOAD_IDENTITY_USER_ROLE,
        principal_set,
        PolicyRead::Post,
        // Exclusive: exactly one repository may impersonate the pusher. An
        // org rename otherwise leaves the old principal able to mint images.
        MemberMode::Exclusive,
    )
    .await
}

/// Outcome of an idempotent IAM member add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingOutcome {
    Added,
    AlreadyPresent,
}

/// The project *number* (not the alphanumeric id) — needed for the GKE
/// puller SA email and the WIF principal set. Reuses the Cloud Resource
/// Manager lookup that already lives in [`iap`]; short-circuits in
/// dry-run, where the synthetic `{}` response carries no `name` to
/// parse.
pub async fn project_number(client: &GcpClient, project_id: &str) -> SetupResult<String> {
    if client.mode() == Mode::DryRun {
        return Ok("000000000000".to_string());
    }
    iap::get_project_number(client, project_id).await
}

/// Shared create-or-409 for the two IAM LRO resources (WIF pool +
/// provider).
async fn create_lro_or_conflict(
    client: &GcpClient,
    service: GcpService,
    path: &str,
    body: &Value,
    operation: &'static str,
) -> SetupResult<EnsureOutcome> {
    let resp = client.post_json(service, path, body).await?;
    match resp.status_u16() {
        200..=299 => {
            let op: Value =
                serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
                    what: "create operation",
                    source,
                })?;
            lro::wait(client, service, &op, "/v1/{name}").await?;
            Ok(EnsureOutcome::Created)
        }
        409 => Ok(EnsureOutcome::AlreadyExists),
        other => Err(SetupError::BadStatus {
            operation: operation.to_string(),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Update an existing IAM LRO resource in place. Paired with
/// [`create_lro_or_conflict`] so an `ensure` converges a resource that already
/// exists with the wrong fields instead of reporting success over it.
async fn patch_lro(
    client: &GcpClient,
    service: GcpService,
    path: &str,
    body: &Value,
    operation: &'static str,
) -> SetupResult<()> {
    let resp = client.patch_json(service, path, body).await?;
    match resp.status_u16() {
        200..=299 => {
            let op: Value =
                serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
                    what: "update operation",
                    source,
                })?;
            lro::wait(client, service, &op, "/v1/{name}").await?;
            Ok(())
        }
        other => Err(SetupError::BadStatus {
            operation: operation.to_string(),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// How a service routes its `:getIamPolicy`. The two surfaces this module
/// touches disagree: Artifact Registry exposes the read as a `GET` on the
/// resource, while IAM's service-account policies want a `POST` with a JSON
/// body. Sending the wrong verb does not return a JSON API error — Google's
/// frontend has no route for it and answers with an HTML `404`, which reads
/// like a missing resource rather than a malformed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyRead {
    Get,
    Post,
}

/// Get→merge→set a single (`role`, `member`) onto `resource`'s IAM
/// policy via the resource's `:getIamPolicy` / `:setIamPolicy` verbs.
/// `read` names the verb the owning service routes the read on; the write
/// is a `POST` on both.
/// Whether a binding may sit alongside others, or must be the only member of
/// its role.
///
/// `Exclusive` exists because an additive `ensure` cannot revoke. The CI pusher
/// trusts exactly one repository, so a stale principal left behind by a rename
/// keeps its ability to impersonate until something removes it — and a
/// provisioner that only ever adds will never be that something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberMode {
    Additive,
    Exclusive,
}

/// The members currently bound to `role`, sorted for comparison.
fn role_members(policy: &Value, role: &str) -> Vec<String> {
    let mut members: Vec<String> = policy["bindings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|binding| binding["role"].as_str() == Some(role))
        .flat_map(|binding| binding["members"].as_array().into_iter().flatten())
        .filter_map(|member| member.as_str().map(ToString::to_string))
        .collect();
    members.sort();
    members.dedup();
    members
}

/// Replace every member of `role` with exactly `member`, leaving other roles
/// untouched.
fn set_sole_member(policy: &mut Value, role: &str, member: &str) {
    // `get_mut`, never `policy["bindings"]`: indexing a `Value` mutably
    // INSERTS a null for a missing key, and `upsert_member` then sees a
    // present-but-null `bindings` and writes a policy with no binding at all.
    if let Some(bindings) = policy.get_mut("bindings").and_then(Value::as_array_mut) {
        bindings.retain(|binding| binding["role"].as_str() != Some(role));
    }
    upsert_member(policy, role, member);
}

async fn ensure_iam_member(
    client: &GcpClient,
    service: GcpService,
    resource: &str,
    role: &str,
    member: &str,
    read: PolicyRead,
    mode: MemberMode,
) -> SetupResult<BindingOutcome> {
    let get_path = format!("{resource}:getIamPolicy");
    let resp = match read {
        PolicyRead::Get => client.get(service, &get_path).await?,
        PolicyRead::Post => client.post_json(service, &get_path, &json!({})).await?,
    };
    let status = resp.status_u16();
    if !(200..=299).contains(&status) {
        return Err(SetupError::BadStatus {
            operation: format!("getIamPolicy for {resource}"),
            status,
            body: resp.into_text(),
        });
    }
    let mut policy: Value =
        serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
            what: "getIamPolicy response",
            source,
        })?;
    match mode {
        MemberMode::Additive => {
            if policy_contains_member(&policy, role, member) {
                return Ok(BindingOutcome::AlreadyPresent);
            }
            upsert_member(&mut policy, role, member);
        }
        MemberMode::Exclusive => {
            if role_members(&policy, role) == [member] {
                return Ok(BindingOutcome::AlreadyPresent);
            }
            set_sole_member(&mut policy, role, member);
        }
    }

    let set_path = format!("{resource}:setIamPolicy");
    let resp = client
        .post_json(service, &set_path, &json!({ "policy": policy }))
        .await?;
    let status = resp.status_u16();
    if !(200..=299).contains(&status) {
        let body = resp.into_text();
        if is_domain_restricted_sharing_refusal(&body) {
            return Err(SetupError::OrgPolicyRefused {
                constraint: DOMAIN_RESTRICTED_SHARING_CONSTRAINT,
                principal: member.to_string(),
                resource: resource.to_string(),
                body,
            });
        }
        return Err(SetupError::BadStatus {
            operation: format!("setIamPolicy for {resource}"),
            status,
            body,
        });
    }
    Ok(BindingOutcome::Added)
}

/// Whether a refused `setIamPolicy` body is a domain-restricted-sharing
/// refusal. GCP words it differently depending on which backend evaluates the
/// policy: some responses name the constraint, others only carry the prose
/// form, so match both rather than the status code alone.
fn is_domain_restricted_sharing_refusal(body: &str) -> bool {
    let lowered = body.to_ascii_lowercase();
    lowered.contains("allowedpolicymemberdomains")
        || lowered.contains("domain restricted sharing")
        || lowered.contains("do not belong to a permitted customer")
}

fn policy_contains_member(policy: &Value, role: &str, member: &str) -> bool {
    policy
        .get("bindings")
        .and_then(Value::as_array)
        .is_some_and(|bindings| {
            bindings.iter().any(|b| {
                b.get("role").and_then(Value::as_str) == Some(role)
                    && b.get("members")
                        .and_then(Value::as_array)
                        .is_some_and(|m| m.iter().any(|x| x.as_str() == Some(member)))
            })
        })
}

fn upsert_member(policy: &mut Value, role: &str, member: &str) {
    // getIamPolicy can return an empty `{}` (no bindings yet); normalize
    // to an object carrying a `bindings` array before merging.
    if !policy.is_object() {
        *policy = json!({});
    }
    let Some(obj) = policy.as_object_mut() else {
        return;
    };
    let bindings = obj
        .entry("bindings".to_string())
        .or_insert_with(|| json!([]));
    let Some(arr) = bindings.as_array_mut() else {
        return;
    };
    for b in arr.iter_mut() {
        if b.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(members) = b.as_object_mut().and_then(|o| {
                o.entry("members".to_string())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
            }) {
                members.push(Value::String(member.to_string()));
            }
            return;
        }
    }
    arr.push(json!({ "role": role, "members": [member] }));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::{GcpClient, GcpService, StaticToken};
    use super::*;

    fn client_for(server: &MockServer, services: &[GcpService]) -> GcpClient {
        let mut c = GcpClient::new(Arc::new(StaticToken("t".into())));
        for s in services {
            c = c.with_base_url(*s, server.uri());
        }
        c
    }

    #[test]
    fn principal_set_pins_the_repository() {
        let p = wif_principal_set("123456789012", "neon-law-source-code/navigator");
        assert!(p.starts_with("principalSet://iam.googleapis.com/projects/123456789012/"));
        assert!(p.ends_with("/attribute.repository/neon-law-source-code/navigator"));
    }

    #[test]
    fn ci_service_account_email_is_project_scoped() {
        assert_eq!(
            ci_service_account_email("navigator-ci-pusher", "my-proj"),
            "navigator-ci-pusher@my-proj.iam.gserviceaccount.com"
        );
    }

    #[tokio::test]
    async fn ensure_repository_posts_docker_format_and_repo_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/p/locations/us-west4/repositories"))
            .and(query_param("repositoryId", "navigator"))
            .and(body_partial_json(json!({ "format": "DOCKER" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server, &[GcpService::ArtifactRegistry]);
        let out = ensure_repository(&client, "p", "us-west4", "navigator")
            .await
            .unwrap();
        assert_eq!(out, EnsureOutcome::Created);
    }

    #[tokio::test]
    async fn ensure_repository_treats_409_as_already_exists() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
            .mount(&server)
            .await;
        let client = client_for(&server, &[GcpService::ArtifactRegistry]);
        let out = ensure_repository(&client, "p", "us-west4", "navigator")
            .await
            .unwrap();
        assert_eq!(out, EnsureOutcome::AlreadyExists);
    }

    /// Retention is a COUNT, not an age: keep the last
    /// [`RETAINED_VERSIONS`] versions of each image and delete the rest.
    ///
    /// Both halves must be in the one PATCH. A `KEEP` policy alone deletes
    /// nothing, and the `DELETE` policy matches every version — `tagState:
    /// ANY` — so shipping it without its `KEEP` partner would empty the
    /// repository on Artifact Registry's next sweep. Keep policies take
    /// precedence over delete policies, which is what makes the pair mean
    /// "keep ten, delete the rest" rather than "delete everything".
    #[tokio::test]
    async fn cleanup_policy_patch_keeps_the_last_ten_versions_and_deletes_the_rest() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator",
            ))
            .and(query_param("updateMask", "cleanupPolicies"))
            .and(body_partial_json(json!({
                "cleanupPolicies": {
                    "keep-last-10-versions": {
                        "id": "keep-last-10-versions",
                        "action": "KEEP",
                        "mostRecentVersions": { "keepCount": 10 }
                    },
                    "delete-unretained-versions": {
                        "id": "delete-unretained-versions",
                        "action": "DELETE",
                        "condition": { "tagState": "ANY" }
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server, &[GcpService::ArtifactRegistry]);
        ensure_cleanup_policy(&client, "p", "us-west4", "navigator")
            .await
            .unwrap();
    }

    /// The age-based rule must not survive alongside the count-based one.
    /// `cleanupPolicies` is a map and the PATCH names it in `updateMask`, so
    /// the request REPLACES the whole set — the retired `delete-older-than-7d`
    /// policy disappears on the next hub setup rather than lingering and
    /// deleting a version the count-based policy meant to keep.
    #[tokio::test]
    async fn the_cleanup_patch_replaces_the_retired_age_based_policy() {
        let server = MockServer::start().await;
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = std::sync::Arc::clone(&captured);
        Mock::given(method("PATCH"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator",
            ))
            .respond_with(move |req: &wiremock::Request| {
                *sink.lock().unwrap() = req.body.clone();
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server, &[GcpService::ArtifactRegistry]);
        ensure_cleanup_policy(&client, "p", "us-west4", "navigator")
            .await
            .unwrap();

        let body = String::from_utf8(captured.lock().unwrap().clone()).expect("utf8 body");
        assert!(
            !body.contains("olderThan") && !body.contains("delete-older-than-7d"),
            "the age-based policy must be gone from the request, got: {body}"
        );
    }

    #[tokio::test]
    async fn ci_service_account_posts_account_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/p/serviceAccounts"))
            .and(body_partial_json(
                json!({ "accountId": "navigator-ci-pusher" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server, &[GcpService::Iam]);
        let out = ensure_ci_service_account(&client, "p", "navigator-ci-pusher")
            .await
            .unwrap();
        assert_eq!(out, EnsureOutcome::Created);
    }

    #[tokio::test]
    async fn repo_iam_member_adds_when_absent_and_skips_when_present() {
        // Absent: get returns an empty policy → set fires.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator:setIamPolicy",
            ))
            .and(body_partial_json(json!({
                "policy": { "bindings": [{ "role": WRITER_ROLE, "members": ["serviceAccount:ci@p.iam.gserviceaccount.com"] }] }
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server, &[GcpService::ArtifactRegistry]);
        let out = ensure_repo_iam_member(
            &client,
            "p",
            "us-west4",
            "navigator",
            WRITER_ROLE,
            "serviceAccount:ci@p.iam.gserviceaccount.com",
        )
        .await
        .unwrap();
        assert_eq!(out, BindingOutcome::Added);

        // Present: get already has the binding → no setIamPolicy mock, so
        // a set call would panic the test.
        let server2 = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "bindings": [{ "role": WRITER_ROLE, "members": ["serviceAccount:ci@p.iam.gserviceaccount.com"] }]
            })))
            .mount(&server2)
            .await;
        let client2 = client_for(&server2, &[GcpService::ArtifactRegistry]);
        let out2 = ensure_repo_iam_member(
            &client2,
            "p",
            "us-west4",
            "navigator",
            WRITER_ROLE,
            "serviceAccount:ci@p.iam.gserviceaccount.com",
        )
        .await
        .unwrap();
        assert_eq!(out2, BindingOutcome::AlreadyPresent);
    }

    /// After the org split a `neon-law` service account is a foreign identity
    /// to a registry in another organization, so this `setIamPolicy` is the call domain
    /// restricted sharing refuses. The operator needs to be told which
    /// constraint and which principal — a bare 403 sends them reading GCP
    /// audit logs to find out.
    #[tokio::test]
    async fn an_org_policy_refusal_names_the_constraint_and_the_principal() {
        for body in [
            r#"{"error":{"code":403,"message":"One or more users named in the policy do not belong to a permitted customer."}}"#,
            r#"{"error":{"code":400,"message":"constraints/iam.allowedPolicyMemberDomains violated"}}"#,
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(
                    "/v1/projects/ghcr/locations/us-west4/repositories/navigator:getIamPolicy",
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path(
                    "/v1/projects/ghcr/locations/us-west4/repositories/navigator:setIamPolicy",
                ))
                .respond_with(ResponseTemplate::new(403).set_body_string(body))
                .mount(&server)
                .await;

            let client = client_for(&server, &[GcpService::ArtifactRegistry]);
            let err = ensure_repo_iam_member(
                &client,
                "ghcr",
                "us-west4",
                "navigator",
                READER_ROLE,
                "serviceAccount:1-compute@developer.gserviceaccount.com",
            )
            .await
            .expect_err("a domain-restricted-sharing refusal must be named, not a bare 403");

            let message = err.to_string();
            assert!(
                message.contains(DOMAIN_RESTRICTED_SHARING_CONSTRAINT),
                "must name the constraint: {message}"
            );
            assert!(
                message.contains("serviceAccount:1-compute@developer.gserviceaccount.com"),
                "must name the refused principal: {message}"
            );
            assert!(
                message.contains("/repositories/navigator"),
                "must name the resource: {message}"
            );
        }
    }

    /// An unrelated non-2xx keeps the existing generic shape, so the named
    /// error stays a signal rather than swallowing every IAM failure.
    #[tokio::test]
    async fn an_unrelated_iam_failure_is_still_a_bare_bad_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator:setIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(500).set_body_string("backend error"))
            .mount(&server)
            .await;

        let client = client_for(&server, &[GcpService::ArtifactRegistry]);
        let err = ensure_repo_iam_member(
            &client,
            "p",
            "us-west4",
            "navigator",
            READER_ROLE,
            "serviceAccount:ci@p.iam.gserviceaccount.com",
        )
        .await
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("500"), "{message}");
        assert!(
            !message.contains(DOMAIN_RESTRICTED_SHARING_CONSTRAINT),
            "{message}"
        );
    }

    #[tokio::test]
    async fn cross_project_reader_binds_the_environment_puller_on_the_hub_repository() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/projects/neon-law"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "name": "projects/123456789012" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/ghcr/locations/us-west4/repositories/navigator:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/ghcr/locations/us-west4/repositories/navigator:setIamPolicy",
            ))
            .and(body_partial_json(json!({
                "policy": { "bindings": [{
                    "role": READER_ROLE,
                    "members": ["serviceAccount:123456789012-compute@developer.gserviceaccount.com"]
                }] }
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(
            &server,
            &[
                GcpService::ArtifactRegistry,
                GcpService::CloudResourceManager,
            ],
        );
        let outcome =
            ensure_cross_project_reader(&client, "neon-law", "ghcr", &SetupConfig::default())
                .await
                .unwrap();
        assert_eq!(outcome, BindingOutcome::Added);
    }

    #[tokio::test]
    async fn wif_provider_posts_tenant_issuer_and_repository_pinned_condition() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/p/locations/global/workloadIdentityPools/github/providers",
            ))
            .and(query_param("workloadIdentityPoolProviderId", "github-oidc"))
            .and(body_partial_json(json!({
                "oidc": { "issuerUri": "https://token.actions.githubusercontent.com" },
                "attributeCondition": "assertion.repository == 'neon-law-source-code/navigator' \
                     && (assertion.ref == 'refs/heads/main' \
                     || assertion.ref.startsWith('refs/tags/'))"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server, &[GcpService::Iam]);
        let out = ensure_wif_provider(&client, "p", "neon-law-source-code/navigator")
            .await
            .unwrap();
        assert_eq!(out, EnsureOutcome::Created);
    }

    /// A host that mints its own OIDC tokens authenticates nothing against a
    /// provider pinned to another host's issuer. The pool and provider already
    /// exist in `ghcr`, so the create path never runs again — only this PATCH
    /// can repair them.
    #[tokio::test]
    async fn wif_provider_converges_an_existing_provider_instead_of_reporting_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/p/locations/global/workloadIdentityPools/github/providers",
            ))
            .respond_with(ResponseTemplate::new(409))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(
                "/v1/projects/p/locations/global/workloadIdentityPools/github/providers/github-oidc",
            ))
            .and(query_param(
                "updateMask",
                "oidc,attributeMapping,attributeCondition",
            ))
            .and(body_partial_json(json!({
                "oidc": { "issuerUri": "https://token.actions.githubusercontent.com" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server, &[GcpService::Iam]);
        let out = ensure_wif_provider(&client, "p", "neon-law-source-code/navigator")
            .await
            .unwrap();
        assert_eq!(out, EnsureOutcome::AlreadyExists);
    }

    /// An org rename leaves the previous principal bound, and an additive
    /// `ensure` never removes it — so the old org keeps the ability to
    /// impersonate the pusher and publish images. Re-running must revoke it.
    #[tokio::test]
    async fn wif_impersonation_revokes_a_stale_principal() {
        // A principal from a pool that no longer federates this repository —
        // the case the revoke exists for. It must differ from `current`, or the
        // test asserts nothing.
        let stale = wif_principal_set("111111111111", "neon-law-source-code/navigator");
        let current = wif_principal_set("522147057781", "neon-law-source-code/navigator");
        assert_ne!(
            stale, current,
            "the stale principal must be a different one"
        );
        let sa = "navigator-ci-deployer@neon-law-stg.iam.gserviceaccount.com";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/v1/projects/neon-law-stg/serviceAccounts/{sa}:getIamPolicy"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "bindings": [{
                    "role": "roles/iam.workloadIdentityUser",
                    "members": [stale, current]
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/v1/projects/neon-law-stg/serviceAccounts/{sa}:setIamPolicy"
            )))
            .and(body_partial_json(json!({
                "policy": { "bindings": [{
                    "role": "roles/iam.workloadIdentityUser",
                    "members": [current]
                }] }
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server, &[GcpService::Iam]);
        let out = ensure_wif_impersonation(&client, "neon-law-stg", sa, &current)
            .await
            .unwrap();
        assert_eq!(out, BindingOutcome::Added);
    }

    #[test]
    fn wif_condition_refuses_a_fork_and_an_arbitrary_branch() {
        let condition = wif_attribute_condition("neon-law-source-code/navigator");
        // Full owner/repo equality, so `attacker/navigator` cannot satisfy it.
        assert!(condition.contains("assertion.repository == 'neon-law-source-code/navigator'"));
        assert!(!condition.contains("repository_owner"));
        // Exactly `main` plus release tags; no other branch mints a token.
        assert!(condition.contains("assertion.ref == 'refs/heads/main'"));
        assert!(condition.contains("assertion.ref.startsWith('refs/tags/')"));
    }

    #[tokio::test]
    async fn dry_run_ensure_records_calls_without_network() {
        let config = SetupConfig {
            region: "us-west4".to_string(),
            ..SetupConfig::default()
        };
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::ArtifactRegistry, "http://127.0.0.1:1")
            .with_base_url(GcpService::Iam, "http://127.0.0.1:1")
            .with_base_url(GcpService::CloudResourceManager, "http://127.0.0.1:1")
            .with_dry_run();
        ensure(&client, "my-project", &config).await.unwrap();
        let calls = client.recorded_calls();
        // repo create + cleanup patch + SA create + writer get + writer set
        // + reader get + reader set + wif pool + wif provider + impersonation
        // get + impersonation set = 11 (project_number is short-circuited in
        // dry-run, no CRM call).
        assert_eq!(calls.len(), 11, "unexpected dry-run calls: {calls:?}");
        assert!(calls
            .iter()
            .any(|c| c.url.contains("/repositories?repositoryId=navigator")));
        assert!(calls
            .iter()
            .any(|c| c.url.contains("workloadIdentityPools/github/providers")));
    }

    /// The two IAM surfaces this module touches route `:getIamPolicy`
    /// differently: Artifact Registry answers a `GET` on the resource, IAM's
    /// service-account policies a `POST`. The wrong verb is not a JSON API
    /// error — Google's frontend has no route for it and returns an HTML
    /// `404`, which reads like a missing repository. Each half below mounts
    /// only the correct verb and answers the other with that same HTML 404,
    /// so a regression fails here instead of at the fifth call of a live
    /// `ops gcp hub setup`.
    #[tokio::test]
    async fn getiampolicy_uses_the_verb_each_service_routes() {
        const GOOGLE_HTML_404: &str =
            "<!DOCTYPE html><html lang=en><title>Error 404 (Not Found)!!1</title>";

        // Artifact Registry: GET carries the read.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/p/locations/us-west4/repositories/navigator:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "bindings": [{ "role": WRITER_ROLE, "members": ["serviceAccount:ci@p.iam.gserviceaccount.com"] }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_string(GOOGLE_HTML_404))
            .mount(&server)
            .await;
        let out = ensure_repo_iam_member(
            &client_for(&server, &[GcpService::ArtifactRegistry]),
            "p",
            "us-west4",
            "navigator",
            WRITER_ROLE,
            "serviceAccount:ci@p.iam.gserviceaccount.com",
        )
        .await
        .expect("Artifact Registry routes :getIamPolicy on GET");
        assert_eq!(out, BindingOutcome::AlreadyPresent);

        // IAM service accounts: POST carries the read.
        let sa_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/p/serviceAccounts/ci@p.iam.gserviceaccount.com:getIamPolicy",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "bindings": [{ "role": WORKLOAD_IDENTITY_USER_ROLE, "members": ["principalSet://example"] }]
            })))
            .expect(1)
            .mount(&sa_server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string(GOOGLE_HTML_404))
            .mount(&sa_server)
            .await;
        let out = ensure_wif_impersonation(
            &client_for(&sa_server, &[GcpService::Iam]),
            "p",
            "ci@p.iam.gserviceaccount.com",
            "principalSet://example",
        )
        .await
        .expect("IAM routes serviceAccounts :getIamPolicy on POST");
        assert_eq!(out, BindingOutcome::AlreadyPresent);
    }
}
