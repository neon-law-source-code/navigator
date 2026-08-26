//! Per-Project publisher identity for Project client-portal bundles.
//!
//! A Project repository's CI publishes its built `portal/dist/` to this
//! deployment's private `<deployment>-applications` bucket, keyless, through
//! Workload Identity Federation. This module provisions the Google half of that
//! trust:
//!
//! 1. A `nav-pub-<code>` service account, **one per Project**. See
//!    [`publisher_account_id`] for why the id carries the Project code verbatim
//!    and what it refuses rather than shortening.
//! 2. A custom role holding exactly `storage.objects.create`,
//!    `storage.objects.update` and `storage.objects.get`, bound on the
//!    applications bucket under an IAM **condition** that confines it to this
//!    Project's own `<code>/portal` prefix. Create *and update*, never delete,
//!    and never another Project's objects.
//! 3. A GitHub OIDC Workload Identity provider pinned to the applications
//!    organization on `main`, issued by
//!    [`GITHUB_OIDC_ISSUER`](super::artifact_registry::GITHUB_OIDC_ISSUER).
//! 4. An impersonation binding pinned to the one `<org>/<repo>` allowed to mint
//!    the publisher's token, so a sibling app repository in the same
//!    organization cannot publish as it.
//!
//! ## Why not `roles/storage.objectCreator`, which this used to grant
//!
//! `objectCreator` is create-only, and the module used to defend that as
//! "create, never delete". The never-delete half is still right and still
//! enforced. The create-*only* half became wrong: the publish `cp`s every object
//! on every run — unconditionally, so that no live asset's age runs out under
//! the bucket's Delete rule — and it stamps `index.html` with custom metadata
//! afterwards. Overwriting an existing object and writing its metadata are
//! `storage.objects.create` and `storage.objects.update`, and a create-only role
//! refuses the second and every republish. A publisher provisioned with
//! `objectCreator` succeeds exactly once and then fails on a permission denial.
//!
//! ## Why a condition, and why the role is custom rather than predefined
//!
//! The bucket is **shared**: every Project's portal lives in it under its own
//! `<code>/portal/` prefix, and the prefix is derived by the Action, not
//! enforced by Google. An unconditioned object-write grant on that bucket
//! therefore lets any Project's CI overwrite every other Project's portal — a
//! privileged client-facing artifact that Navigator serves same-origin. The
//! condition is what makes the derived prefix an enforced one.
//!
//! No predefined role is create-and-update without delete: `objectCreator` is
//! create-only, `objectUser` and `objectAdmin` both carry delete. So the role is
//! custom and holds three permissions. It deliberately does **not** hold
//! `storage.objects.list`: listing is evaluated against the *bucket*, so no
//! object-name condition can scope it, and a grant of it would leak every other
//! Project's object names. The publish does not need it — it uses `cp`, which
//! never lists.
//!
//! A condition lives on a binding, and a binding names one role and one member
//! set, so **a publisher account carries exactly one prefix**. One publisher
//! identity per Project is a consequence of this shape, not a preference, and
//! the account id is therefore derived from the Project code rather than from
//! the GCP project id alone. [`ambiguous_repoint`] is what a *shared* account
//! hit on the second Project; with one account per Project it is reachable only
//! after a genuine repository rename.
//!
//! The deployment-level half — the custom role, the Workload Identity pool, and
//! its provider — is provisioned once per run rather than once per Project, so
//! a four-Project deployment POSTs one role and one provider. Only the account,
//! the conditioned bucket binding, and the impersonation are per Project.
//!
//! The consumer half — the composite Action a Project repository runs — is
//! `.github/actions/application-publish`; the provider resource and the service
//! account email are set on each Project repository as repository *secrets*
//! (public identifiers, but they name the deployment's GCP project in a public
//! log; the trust lives in the binding here). See
//! `docs/project-repositories.md`.
//!
//! Everything is idempotent on the pipeline convention: creates POST
//! unconditionally and treat HTTP 409 as success, and the impersonation binding
//! is get-merge-set. The provider mirrors the marketing deploy identity in
//! `marketing.rs`; the impersonation reuses
//! [`ensure_wif_impersonation`](super::artifact_registry::ensure_wif_impersonation),
//! whose exclusive mode revokes a principal a repository rename left behind.

use cloud::workspace::{is_valid_slug, SLUG_MAX_LEN};
use serde_json::{json, Value};

use super::artifact_registry::{ensure_wif_impersonation, project_number, GITHUB_OIDC_ISSUER};
use super::client::{GcpClient, GcpService};
use super::error::{SetupError, SetupResult};
use super::lro;

/// The hard GCP ceiling on a service-account id, the local part of its email.
///
/// Google requires 6-30 characters, starting with a lowercase letter and ending
/// alphanumeric. This is the first place in the pipeline that asserts it: no
/// other account id here is derived from anything, so none of them could ever
/// overflow.
pub const ACCOUNT_ID_MAX_LEN: usize = 30;

/// Everything in a publisher's account id that is not the Project code.
///
/// Read as *whose* (`nav`), *what* (`pub`), *which Project* (the code) — the
/// same owner-first order as the sibling `navigator-web`, `navigator-drive` and
/// `navigator-ci-pusher` accounts, shortened because 30 characters is the whole
/// budget and `navigator-app-publisher-` alone spent 24 of them.
///
/// Three properties are load-bearing rather than aesthetic:
///
/// * **It is an instance of `nav-<role>-<code>`, not a name.** A second kind of
///   per-Project identity gets a slot in the same scheme instead of inventing a
///   second convention.
/// * **It clears the 6-character minimum unconditionally.** The shortest valid
///   Project code is one character, and `nav-pub-a` is nine, so a too-*short*
///   id is unreachable and there is no branch for it to hide in.
/// * **It starts with a lowercase letter.** [`cloud::workspace::is_valid_slug`]
///   admits a leading digit, which GCP does not, so the prefix is also what
///   makes every derived id well-formed.
///
/// `pub-` alone was rejected: in a deployment that also holds a *public* assets
/// bucket it reads as "public" at least as readily as "publisher", and the one
/// thing this account must never be is public.
pub const PUBLISHER_ACCOUNT_PREFIX: &str = "nav-pub-";

/// The longest Project code that fits inside a publisher account id.
///
/// Twenty-two characters, and it is a real ceiling rather than a soft target: a
/// longer code is refused by [`publisher_account_id`], never shortened. See
/// there for why no prefix makes this problem go away.
pub const PUBLISHER_CODE_MAX_LEN: usize = ACCOUNT_ID_MAX_LEN - PUBLISHER_ACCOUNT_PREFIX.len();

/// Workload Identity pool the Project repositories federate through. Distinct
/// from the registry's `github` pool, which environment projects never create.
pub const APP_PUBLISHER_WIF_POOL_ID: &str = "app-publisher";
/// The provider id the consumer Action expects in the resource it is passed.
///
/// **The `ghe-oidc` spelling stays, and this is not stale narration.** It is a
/// live resource id, and a provider id is not patchable: renaming it here would
/// ask for a *second* provider under the same pool rather than renaming the
/// first, while `create_lro_or_conflict` read the 409 from the existing one as
/// success. The rename would report success and converge nothing — and
/// `.github/actions/application-publish/action.yml` names this id in the
/// resource it documents, so the two would disagree.
pub const APP_PUBLISHER_WIF_PROVIDER_ID: &str = "ghe-oidc";

/// Id of the custom role the publisher holds on the applications bucket.
pub const PUBLISHER_ROLE_ID: &str = "navigatorApplicationsPublisher";

/// The permissions that custom role holds, and the complete set.
///
/// `create` overwrites an object (a GCS overwrite is a new generation, not a
/// delete), `update` writes the custom metadata the publish stamps onto
/// `index.html`, and `get` covers the destination probe gcloud performs before
/// writing. `storage.objects.delete` is absent because the publish never
/// deletes, and `storage.objects.list` is absent because listing is evaluated
/// against the bucket and no object-name condition can scope it.
pub const PUBLISHER_PERMISSIONS: &[&str] = &[
    "storage.objects.create",
    "storage.objects.get",
    "storage.objects.update",
];

/// Roles a publisher may hold from an earlier provisioning round, which `ensure`
/// strips off the bucket policy so the narrowed grant is not additive to a wider
/// one left in place.
///
/// `objectCreator` is what this module granted before; `objectAdmin` is what
/// production was hand-patched to when create-only refused a republish. Leaving
/// either alongside the conditioned binding would keep the hole open, since IAM
/// is a union of grants.
const SUPERSEDED_PUBLISHER_ROLES: &[&str] = &[
    "roles/storage.objectCreator",
    "roles/storage.objectAdmin",
    "roles/storage.objectUser",
];

/// The full resource name of the publisher's custom role in `project_id`.
#[must_use]
pub fn publisher_role_name(project_id: &str) -> String {
    format!("projects/{project_id}/roles/{PUBLISHER_ROLE_ID}")
}

/// The IAM condition confining the publisher to one Project's portal prefix.
///
/// Two clauses, and both are needed. The `startsWith` clause covers every object
/// under the prefix; the equality clause covers the prefix path *itself*, which
/// gcloud probes as though it were an object before writing — without it that
/// probe is denied and the publish fails before uploading anything.
///
/// `code` is the Project code, which is also the repository name: the Action
/// derives the object prefix from `github.event.repository.name`, so the
/// condition and the upload path are derived from the same string.
#[must_use]
pub fn publisher_condition_expression(bucket: &str, code: &str) -> String {
    let prefix = format!("projects/_/buckets/{bucket}/objects/{code}/portal");
    format!("resource.name == \"{prefix}\" || resource.name.startsWith(\"{prefix}/\")")
}

/// The account id of `code`'s publisher, or a refusal if it cannot be derived.
///
/// The Project code travels **verbatim**, and the three alternatives were all
/// rejected on trust rather than on length:
///
/// * **Truncation collides silently.** Two codes sharing the first 22 bytes fold
///   onto one account, and the second provisioning run repoints the first
///   Project's conditioned binding — exactly the failure
///   [`ambiguous_repoint`] exists to refuse, arriving through a path that never
///   reaches it.
/// * **A hash is unauditable.** The id is the only human-readable link between a
///   principal in a bucket IAM policy and a portal prefix, and matching those by
///   eye is the one audit anyone can actually perform on that policy.
/// * **An ordinal would be positional.** `nav-pub-1` never collides and is
///   always short, but its meaning would live in the order of a configuration
///   list, so reordering that list silently repoints every account after the
///   edit.
///
/// **No prefix makes the ceiling go away, so the ceiling is not the axis to
/// optimize.** Codes are client-derived, and a `surname-mattertype-jurisdiction`
/// shape reaches 29 characters — which overflows a 30-character id even with a
/// zero-length prefix. Shortening `nav-pub-` further would buy headroom only in
/// the narrow band a code of 23 to 27 characters occupies, and would pay for it
/// with an illegible principal and a convention that cannot extend. So the
/// prefix optimizes for legibility and the overflow is refused here, by name.
///
/// The shape check is [`cloud::workspace::is_valid_slug`] rather than a second
/// regular expression: a Project code, its repository name and this account id
/// are one shape defined once.
pub fn publisher_account_id(code: &str) -> SetupResult<String> {
    if !is_valid_slug(code) {
        return Err(SetupError::PublisherCodeRefused {
            code: code.to_string(),
            reason: format!(
                "it is not a well-formed Project code: lowercase letters, digits and single \
                 hyphens, alphanumeric at both ends, at most {SLUG_MAX_LEN} characters",
            ),
        });
    }
    if code.len() > PUBLISHER_CODE_MAX_LEN {
        return Err(SetupError::PublisherCodeRefused {
            code: code.to_string(),
            reason: format!(
                "its publisher account id `{PUBLISHER_ACCOUNT_PREFIX}{code}` would be {len} \
                 characters and GCP allows at most {ACCOUNT_ID_MAX_LEN}. The code is carried \
                 verbatim because a shortened one collides silently with every other code \
                 sharing its first {PUBLISHER_CODE_MAX_LEN} bytes, so shorten the Project \
                 code itself — it is also the repository name and the bucket prefix — to at \
                 most {PUBLISHER_CODE_MAX_LEN} characters",
                len = PUBLISHER_ACCOUNT_PREFIX.len() + code.len(),
            ),
        });
    }
    Ok(format!("{PUBLISHER_ACCOUNT_PREFIX}{code}"))
}

/// One Project's publisher identity, resolved and validated before any call.
///
/// Exists so the derivation is fallible exactly once, at the top of [`ensure`],
/// and the provisioning loop below it carries strings that are already known to
/// be well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherIdentity {
    /// The Project code, which is also the repository name and the object prefix.
    pub code: String,
    /// Local part of the service-account email.
    pub account_id: String,
    /// The full service-account email.
    pub email: String,
}

/// Resolve every Project's publisher identity, or refuse before provisioning any.
///
/// **This runs before the first POST, and that ordering is the point.** Checking
/// each code as the loop reaches it would leave a deployment whose third code is
/// too long with two accounts created, two bucket bindings written and two
/// impersonations bound — a partial apply an operator then has to reason about.
/// Refusing up front leaves nothing behind, and because the check is pure it
/// also fires under `--dry-run` and in a test with no credentials.
pub fn resolve_publishers(
    project_id: &str,
    codes: &[String],
) -> SetupResult<Vec<PublisherIdentity>> {
    codes
        .iter()
        .map(|code| {
            let account_id = publisher_account_id(code)?;
            Ok(PublisherIdentity {
                email: format!("{account_id}@{project_id}.iam.gserviceaccount.com"),
                code: code.clone(),
                account_id,
            })
        })
        .collect()
}

/// The full provider resource the deployment publishes as
/// `NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER`.
#[must_use]
pub fn wif_provider_resource(project_number: &str) -> String {
    format!(
        "projects/{project_number}/locations/global/workloadIdentityPools/\
         {APP_PUBLISHER_WIF_POOL_ID}/providers/{APP_PUBLISHER_WIF_PROVIDER_ID}"
    )
}

/// The attribute condition guarding token exchange: the applications org, on
/// `main` only. Pinned to `repository_owner` so every app repository in the org
/// can reach the impersonation gate, and to `refs/heads/main` so no other ref
/// mints a token.
#[must_use]
pub fn wif_attribute_condition(org: &str) -> String {
    format!("assertion.repository_owner == '{org}' && assertion.ref == 'refs/heads/main'")
}

/// The bucket IAM endpoint, read and written at **policy version 3**.
///
/// `optionsRequestedPolicyVersion=3` is not optional here, and omitting it does
/// not merely hide a field. Asked for a version 1 policy — the default — IAM
/// renders each conditional binding with the condition *dropped* and the role
/// mangled to `<role>_withcond_<hash>`. This module would then fail to
/// recognise its own converged binding, append a duplicate beside the mangled
/// one, and PUT a policy naming a role that does not exist. Every run after the
/// first would write, and be rejected.
///
/// The same path carries the PUT, so the write is version 3 on both halves.
#[must_use]
pub fn bucket_iam_path(bucket: &str) -> String {
    format!("/storage/v1/b/{bucket}/iam?optionsRequestedPolicyVersion=3")
}

/// The impersonation principal set, pinned to one `<org>/<repo>` so a sibling
/// app repository cannot mint the publisher's token even though the provider
/// trusts the whole organization.
#[must_use]
pub fn wif_principal_set(project_number: &str, org: &str, repo: &str) -> String {
    format!(
        "principalSet://iam.googleapis.com/projects/{project_number}/locations/global/\
         workloadIdentityPools/{APP_PUBLISHER_WIF_POOL_ID}/attribute.repository/{org}/{repo}"
    )
}

/// Provision one publisher identity per Project repository in `repos`.
///
/// `org` is the one applications organization this deployment federates, and it
/// stays singular because the Workload Identity provider's `attributeCondition`
/// names exactly one `repository_owner` and there is one provider per
/// deployment. Each entry in `repos` is a repository name, which *is* the
/// Project code the bucket condition is scoped to — the Action derives its
/// object prefix from the same repository name, so a rename moves both together
/// or neither.
///
/// The applications bucket must already exist — [`ensure_publisher_grant`]
/// binds each publisher on it — which is why `run` calls this after the buckets
/// stage.
///
/// Every code is resolved before the first call ([`resolve_publishers`]), and
/// the three deployment-level resources are provisioned once rather than once
/// per Project.
pub async fn ensure(
    client: &GcpClient,
    project_id: &str,
    org: &str,
    repos: &[String],
    applications_bucket: &str,
) -> SetupResult<()> {
    let publishers = resolve_publishers(project_id, repos)?;

    // Deployment-level, and hoisted out of the loop below: one custom role
    // definition, one pool and one provider serve every Project here.
    ensure_publisher_role(client, project_id).await?;
    ensure_wif_pool(client, project_id).await?;
    ensure_wif_provider(client, project_id, org).await?;
    let number = project_number(client, project_id).await?;

    // The coordinates each Project repository sets as repository *secrets*.
    // Printed so an operator can copy them straight into each repository's
    // Actions secrets. They are public identifiers and the trust is enforced by
    // the bindings below, so neither is key material — but both name the
    // deployment's GCP project, and a Project repository's Actions log is
    // public, so they are secrets to keep them out of it rather than because
    // they are sensitive. See docs/project-repositories.md and
    // `.github/actions/application-publish/action.yml`.
    //
    // The provider resource is one per deployment, so it is printed once even
    // though every Project repository sets it as its own secret. The service
    // account differs per Project and is printed inside the loop.
    eprintln!(
        "gcp setup [{project_id}] set repository secret \
         NAVIGATOR_APP_PUBLISHER_WIF_PROVIDER={}",
        wif_provider_resource(&number)
    );

    for publisher in &publishers {
        let PublisherIdentity { code, email, .. } = publisher;
        ensure_publisher_account(client, project_id, publisher).await?;
        ensure_publisher_grant(client, project_id, applications_bucket, code, email).await?;
        ensure_wif_impersonation(
            client,
            project_id,
            email,
            &wif_principal_set(&number, org, code),
        )
        .await?;
        eprintln!(
            "gcp setup [{project_id}] set repository secret on {org}/{code} \
             NAVIGATOR_APP_PUBLISHER_SERVICE_ACCOUNT={email}"
        );
    }
    Ok(())
}

/// Idempotently create one Project's publisher service account.
/// `serviceAccounts.create` returns the finished account rather than a
/// long-running operation, so this must not be routed through an LRO wait.
///
/// The `displayName` and `description` carry the Project code unabridged. An
/// account id is capped at 30 characters and a display name at 100, so this is
/// where the console gets something to list and a reader gets prose — the id
/// does not have to carry the whole burden of legibility. It carries the code
/// anyway, because the id is what appears in a bucket IAM policy and the
/// display name is not.
async fn ensure_publisher_account(
    client: &GcpClient,
    project_id: &str,
    publisher: &PublisherIdentity,
) -> SetupResult<()> {
    let PublisherIdentity {
        code, account_id, ..
    } = publisher;
    let path = format!("/v1/projects/{project_id}/serviceAccounts");
    let body = json!({
        "accountId": account_id,
        "serviceAccount": {
            "displayName": format!("Navigator application publisher — {code}"),
            "description": format!(
                "Publishes the {code} Project's portal bundle to the {code}/portal prefix of \
                 the applications bucket. One publisher per Project: a condition lives on a \
                 binding, so one account carries exactly one prefix.",
            ),
        },
    });
    let resp = client.post_json(GcpService::Iam, &path, &body).await?;
    match resp.status_u16() {
        200..=299 | 409 => Ok(()),
        other => Err(SetupError::BadStatus {
            operation: format!("create service account {account_id}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Idempotently create the publisher's custom role, holding exactly
/// [`PUBLISHER_PERMISSIONS`].
///
/// A project-level role definition, bound on the bucket by
/// [`ensure_publisher_grant`] — defining a role grants nothing on its own.
/// `roles.create` returns the finished role rather than a long-running
/// operation. A 409 means the id is taken, which is *not* the same as the role
/// being usable — see [`refuse_if_role_is_soft_deleted`].
///
/// A 409 is *not* followed by a PATCH. The permission set is the contract this
/// module asserts, and silently widening a role an operator narrowed by hand
/// would be the same class of surprise as the hand-patch this change exists to
/// reconcile. A role whose permissions have drifted is a reconcile decision, not
/// a create-path side effect.
async fn ensure_publisher_role(client: &GcpClient, project_id: &str) -> SetupResult<()> {
    let path = format!("/v1/projects/{project_id}/roles?roleId={PUBLISHER_ROLE_ID}");
    let body = json!({
        "role": {
            "title": "Navigator applications publisher",
            "description": "Create and update objects under one Project's portal prefix; \
                            never delete, never list.",
            "includedPermissions": PUBLISHER_PERMISSIONS,
            "stage": "GA",
        },
    });
    let resp = client.post_json(GcpService::Iam, &path, &body).await?;
    match resp.status_u16() {
        200..=299 => Ok(()),
        409 => refuse_if_role_is_soft_deleted(client, project_id).await,
        other => Err(SetupError::BadStatus {
            operation: format!("create custom role {PUBLISHER_ROLE_ID}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Distinguish the two things a 409 on `roles.create` means.
///
/// A custom role is **soft-deleted** for seven days before its id is free
/// again, and during that window `roles.create` still answers 409 while the
/// role stays `deleted: true`. A binding naming a deleted role is inert, so
/// treating that 409 as "already exists" would provision a publisher that
/// authenticates, holds a binding, and can write nothing — the failure arriving
/// later, in a Project repository's CI, far from its cause.
///
/// Undeleting is not done here for the same reason a 409 is not followed by a
/// PATCH: restoring a role an operator deleted is their decision, not a
/// create-path side effect. This only refuses to report success.
async fn refuse_if_role_is_soft_deleted(client: &GcpClient, project_id: &str) -> SetupResult<()> {
    let path = format!("/v1/projects/{project_id}/roles/{PUBLISHER_ROLE_ID}");
    let resp = client.get(GcpService::Iam, &path).await?;
    let status = resp.status_u16();
    if !(200..=299).contains(&status) {
        return Err(SetupError::BadStatus {
            operation: format!("read custom role {PUBLISHER_ROLE_ID} after a 409"),
            status,
            body: resp.into_text(),
        });
    }
    let role: Value =
        serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
            what: "custom role",
            source,
        })?;
    if role.get("deleted").and_then(Value::as_bool) == Some(true) {
        return Err(SetupError::AmbiguousLiveState {
            operation: format!("create custom role {PUBLISHER_ROLE_ID}"),
            detail: format!(
                "the role exists but is soft-deleted, so its id is taken and a binding naming \
                 it would grant nothing. Undelete it — `gcloud iam roles undelete \
                 {PUBLISHER_ROLE_ID} --project {project_id}` — or wait out the seven-day \
                 window for the id to free, then re-run.",
            ),
        });
    }
    Ok(())
}

/// Bind the publisher's custom role on the applications bucket, under a
/// condition confining it to `code`'s own portal prefix, and strip any
/// superseded wider grant the same publisher still holds.
///
/// Get-merge-put against the live policy, and it does three things rather than
/// one:
///
/// 1. Removes the publisher from any [`SUPERSEDED_PUBLISHER_ROLES`] binding, and
///    drops a binding left with no members. IAM is a union, so adding the narrow
///    grant beside a wide one narrows nothing — this is what reconciles a
///    deployment hand-patched to `objectAdmin`.
/// 2. Ensures exactly one conditioned binding for the custom role, and refuses
///    outright if the publisher already carries a *different* prefix — see the
///    branch itself for why that ambiguity is not safe to resolve by guessing.
/// 3. Sets `version: 3`, without which a conditioned binding is rejected. The
///    fetched `etag` travels back untouched, so a concurrent edit loses rather
///    than being silently overwritten.
///
/// Both halves speak version 3: the read goes through [`bucket_iam_path`],
/// because a version 1 read hides the very condition this function is trying to
/// recognise.
///
/// It writes only when something actually changed, so a converged re-run makes
/// no request — the property `navigator ops gcp setup` is expected to have.
async fn ensure_publisher_grant(
    client: &GcpClient,
    project_id: &str,
    bucket: &str,
    code: &str,
    publisher_email: &str,
) -> SetupResult<()> {
    let member = format!("serviceAccount:{publisher_email}");
    let role = publisher_role_name(project_id);
    let expression = publisher_condition_expression(bucket, code);
    let path = bucket_iam_path(bucket);

    let response = client.get(GcpService::Storage, &path).await?;
    let status = response.status_u16();
    if !(200..=299).contains(&status) {
        return Err(SetupError::BadStatus {
            operation: format!("read IAM policy for bucket {bucket}"),
            status,
            body: response.into_text(),
        });
    }
    let mut policy: Value =
        serde_json::from_str(&response.into_text()).map_err(|source| SetupError::Json {
            what: "bucket IAM policy",
            source,
        })?;

    let bindings = policy
        .get("bindings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let Merged { next, changed } = merge_publisher_bindings(MergeInputs {
        bindings,
        member: &member,
        role: &role,
        expression: &expression,
        bucket,
        code,
    })?;

    let needs_version = policy.get("version").and_then(Value::as_i64) != Some(3);
    if !changed && !needs_version {
        return Ok(());
    }

    policy["bindings"] = Value::Array(next);
    policy["version"] = json!(3);

    let resp = client.put_json(GcpService::Storage, &path, &policy).await?;
    match resp.status_u16() {
        200..=299 => Ok(()),
        other => Err(SetupError::BadStatus {
            operation: format!("grant publisher role on bucket {bucket}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// What [`merge_publisher_bindings`] needs, grouped so the call keeps one
/// argument per idea rather than six positional strings.
struct MergeInputs<'a> {
    bindings: Vec<Value>,
    /// `serviceAccount:<publisher email>`, the member every branch keys on.
    member: &'a str,
    /// Full resource name of the publisher's custom role.
    role: &'a str,
    /// The condition this run wants bound.
    expression: &'a str,
    bucket: &'a str,
    code: &'a str,
}

/// The merged binding list, and whether it differs from what was fetched.
struct Merged {
    next: Vec<Value>,
    changed: bool,
}

/// Merge the publisher's conditioned binding into `bindings`, stripping any
/// superseded wide grant and refusing an ambiguous repoint.
///
/// Split out of [`ensure_publisher_grant`] so the transport half — read, decide
/// whether to write, write — stays readable beside the policy algebra. Pure: it
/// makes no request, which is also what makes every branch testable through
/// [`ensure_publisher_grant`] without a live bucket.
fn merge_publisher_bindings(inputs: MergeInputs<'_>) -> SetupResult<Merged> {
    let MergeInputs {
        bindings,
        member,
        role,
        expression,
        bucket,
        code,
    } = inputs;

    let mut changed = false;
    let mut already_bound = false;
    let mut next: Vec<Value> = Vec::with_capacity(bindings.len() + 1);

    for binding in bindings {
        let binding_role = binding.get("role").and_then(Value::as_str).unwrap_or("");
        let holds_publisher = binding
            .get("members")
            .and_then(Value::as_array)
            .is_some_and(|members| members.iter().any(|m| m.as_str() == Some(member)));

        if holds_publisher && SUPERSEDED_PUBLISHER_ROLES.contains(&binding_role) {
            // Strip the publisher out; keep any other member of that binding.
            let remaining: Vec<Value> = binding
                .get("members")
                .and_then(Value::as_array)
                .map(|members| {
                    members
                        .iter()
                        .filter(|m| m.as_str() != Some(member))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            changed = true;
            if !remaining.is_empty() {
                let mut kept = binding.clone();
                kept["members"] = Value::Array(remaining);
                next.push(kept);
            }
            continue;
        }

        if binding_role == role && holds_publisher {
            let bound_to = binding
                .get("condition")
                .and_then(|c| c.get("expression"))
                .and_then(Value::as_str);
            if bound_to == Some(expression) {
                already_bound = true;
                next.push(binding);
                continue;
            }
            return Err(ambiguous_repoint(
                member, bucket, code, bound_to, expression,
            ));
        }

        next.push(binding);
    }

    if !already_bound {
        next.push(json!({
            "role": role,
            "members": [member],
            "condition": {
                "title": "one Project's portal prefix",
                "description":
                    "Confines the publisher to this Project's own `<code>/portal` prefix in \
                     the shared applications bucket.",
                "expression": expression,
            },
        }));
        changed = true;
    }

    Ok(Merged { next, changed })
}

/// The refusal raised when the publisher already carries a different prefix.
///
/// Two situations produce an identical policy and only one is safe to converge:
///
///   * the Project repository was **renamed**, so the old prefix is dead and
///     replacing the condition is exactly right; or
///   * a **second Project** is being provisioned against this same publisher,
///     because the account is derived from the GCP project id alone and every
///     Project in the deployment shares it. Repointing there revokes the first
///     Project's publish, and reports success doing it.
///
/// Nothing in the policy distinguishes them, so this refuses rather than
/// guesses: "rename" is the destructive reading, a rename is rare and
/// operator-driven, and the collision is what rolling out a second Project
/// actually hits. Both prefixes are named so an operator can tell which it is
/// without reading the bucket policy by hand.
fn ambiguous_repoint(
    member: &str,
    bucket: &str,
    code: &str,
    bound_to: Option<&str>,
    wanted: &str,
) -> SetupError {
    SetupError::AmbiguousLiveState {
        operation: format!("bind the applications publisher to {code}'s portal prefix"),
        detail: format!(
            "{member} is already bound to a different prefix on bucket {bucket}.\n  \
             bound:   {bound}\n  \
             wanted:  {wanted}\n\
             A condition lives on a binding, so one publisher account carries exactly one \
             prefix, and this account is shared by every Project in the deployment. If the \
             repository was renamed, remove the stale binding by hand and re-run. If this is a \
             second Project, it needs its own publisher identity — one account cannot isolate \
             two portals.",
            bound = bound_to.unwrap_or("<no condition>"),
        ),
    }
}

/// Idempotently create the app-publisher Workload Identity pool.
async fn ensure_wif_pool(client: &GcpClient, project_id: &str) -> SetupResult<()> {
    let path = format!(
        "/v1/projects/{project_id}/locations/global/workloadIdentityPools\
         ?workloadIdentityPoolId={APP_PUBLISHER_WIF_POOL_ID}"
    );
    let body = json!({ "displayName": "Navigator application publisher" });
    create_lro_or_conflict(client, &path, &body, "create app-publisher WIF pool").await
}

/// Idempotently create the GitHub OIDC provider under the app-publisher pool,
/// pinned to the applications organization on `main`.
///
/// The provider id is [`APP_PUBLISHER_WIF_PROVIDER_ID`], still spelled
/// `ghe-oidc`. That spelling is a live resource id and not narration — see the
/// constant for why renaming it would converge nothing.
async fn ensure_wif_provider(client: &GcpClient, project_id: &str, org: &str) -> SetupResult<()> {
    let path = format!(
        "/v1/projects/{project_id}/locations/global/workloadIdentityPools/\
         {APP_PUBLISHER_WIF_POOL_ID}/providers\
         ?workloadIdentityPoolProviderId={APP_PUBLISHER_WIF_PROVIDER_ID}"
    );
    // The "GitHub Enterprise OIDC" display name is stale — these repositories
    // are on github.com — and it is deliberately left. `create_lro_or_conflict`
    // POSTs and reads a 409 as done; there is no PATCH path here, so a rename
    // would apply to providers created *after* it and to none of the ones that
    // exist. Correcting it means adding convergence first, which is a change to
    // live infrastructure rather than to a name (ENG-284 category 2).
    let body = json!({
        "displayName": "GitHub Enterprise OIDC",
        "oidc": { "issuerUri": GITHUB_OIDC_ISSUER },
        "attributeMapping": {
            "google.subject": "assertion.sub",
            "attribute.repository": "assertion.repository",
            "attribute.repository_owner": "assertion.repository_owner"
        },
        "attributeCondition": wif_attribute_condition(org)
    });
    create_lro_or_conflict(client, &path, &body, "create app-publisher WIF provider").await
}

/// POST an IAM create, waiting on any long-running operation and treating 409 as
/// already done.
async fn create_lro_or_conflict(
    client: &GcpClient,
    path: &str,
    body: &Value,
    operation: &'static str,
) -> SetupResult<()> {
    let resp = client.post_json(GcpService::Iam, path, body).await?;
    match resp.status_u16() {
        200..=299 => {
            let op: Value =
                serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
                    what: "create operation",
                    source,
                })?;
            lro::wait(client, GcpService::Iam, &op, "/v1/{name}").await?;
            Ok(())
        }
        409 => Ok(()),
        other => Err(SetupError::BadStatus {
            operation: operation.to_string(),
            status: other,
            body: resp.into_text(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::{GcpService, StaticToken};
    use super::*;

    fn offline_dry_run_client() -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Iam, "http://127.0.0.1:1")
            .with_base_url(GcpService::Storage, "http://127.0.0.1:1")
            .with_base_url(GcpService::CloudResourceManager, "http://127.0.0.1:1")
            .with_dry_run()
    }

    #[test]
    fn the_account_id_carries_the_project_code_verbatim() {
        assert_eq!(
            publisher_account_id("sample-litigation").unwrap(),
            "nav-pub-sample-litigation",
        );
    }

    /// The ceiling is exactly 22 characters of Project code, and it is derived
    /// rather than typed: 30 minus the prefix. A change to the prefix moves the
    /// ceiling with it, which is why nothing here hardcodes 22 twice.
    #[test]
    fn the_code_ceiling_is_thirty_minus_the_prefix() {
        assert_eq!(PUBLISHER_ACCOUNT_PREFIX, "nav-pub-");
        assert_eq!(PUBLISHER_CODE_MAX_LEN, 22);
        assert_eq!(
            PUBLISHER_ACCOUNT_PREFIX.len() + PUBLISHER_CODE_MAX_LEN,
            ACCOUNT_ID_MAX_LEN,
        );
    }

    /// Every account id the scheme can produce is a well-formed GCP one.
    ///
    /// This is the property the prefix was chosen for, and it is asserted at
    /// both ends of the code-length range rather than argued in a comment. The
    /// 6-character minimum is unreachable because the prefix alone is 8, and the
    /// leading character is a letter even though
    /// [`cloud::workspace::is_valid_slug`] would admit a code starting with a
    /// digit.
    #[test]
    fn every_derived_account_id_is_a_valid_gcp_account_id() {
        let longest = "a".repeat(PUBLISHER_CODE_MAX_LEN);
        for code in ["a", "9", "0-a", "sample-transactional", longest.as_str()] {
            let id = publisher_account_id(code)
                .unwrap_or_else(|e| panic!("`{code}` must be accepted: {e}"));
            assert!(
                (6..=ACCOUNT_ID_MAX_LEN).contains(&id.len()),
                "`{id}` is {} characters, outside GCP's 6-30",
                id.len(),
            );
            let first = id.as_bytes()[0];
            assert!(
                first.is_ascii_lowercase(),
                "`{id}` must start with a lowercase letter, not `{}`",
                first as char,
            );
            assert!(
                id.as_bytes()
                    .last()
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
                "`{id}` must end alphanumeric",
            );
            assert!(!id.contains("--"), "`{id}` must not carry a double hyphen");
        }
    }

    /// A code one character past the ceiling is refused, and never shortened.
    ///
    /// Shortening is the failure this refusal exists to prevent: two codes
    /// sharing their first 22 bytes would fold onto one account, and the second
    /// provisioning run would repoint the first Project's conditioned binding —
    /// reaching the outcome [`ambiguous_repoint`] refuses, by a path that never
    /// consults it.
    #[test]
    fn a_code_past_the_ceiling_is_refused_rather_than_shortened() {
        let too_long = "a".repeat(PUBLISHER_CODE_MAX_LEN + 1);
        let err = publisher_account_id(&too_long)
            .expect_err("a code past the ceiling must not produce an account id");
        assert!(
            matches!(err, SetupError::PublisherCodeRefused { .. }),
            "{err}",
        );
        let message = err.to_string();
        // The refusal names the ceiling and what to do, because the code is also
        // the repository name and the bucket prefix — an operator changes it
        // once, at Project creation, or not at all.
        assert!(
            message.contains("30") && message.contains("22"),
            "the refusal must name both the GCP limit and the code ceiling: {message}",
        );
        assert!(
            message.contains(&too_long),
            "the refusal must name the offending code: {message}",
        );
    }

    /// A code that is not a well-formed Project code is refused too.
    ///
    /// The shape check is [`cloud::workspace::is_valid_slug`] rather than a
    /// second regular expression, so an uppercase or double-hyphenated value
    /// cannot reach GCP as an account id at all.
    #[test]
    fn a_malformed_code_never_becomes_an_account_id() {
        for code in [
            "Sample",
            "sample--estate",
            "-sample",
            "sample-",
            "samp le",
            "",
        ] {
            let err =
                publisher_account_id(code).expect_err("a malformed Project code must be refused");
            assert!(
                matches!(err, SetupError::PublisherCodeRefused { .. }),
                "`{code}`: {err}",
            );
        }
    }

    /// The whole list is validated before the first call, not as the loop
    /// reaches each entry.
    ///
    /// A refusal on the third of four codes must leave nothing provisioned. This
    /// asserts the pure half — that `resolve_publishers` fails as a unit — and
    /// [`ensure`] calls it before its first request, which
    /// `a_too_long_code_refuses_before_any_call_is_made` proves against a
    /// recording client.
    #[test]
    fn resolve_publishers_refuses_the_whole_list_or_none_of_it() {
        let ok = resolve_publishers(
            "neon-law-stg",
            &["sample-estate".to_string(), "sample-litigation".to_string()],
        )
        .expect("both codes fit");
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].account_id, "nav-pub-sample-estate");
        assert_eq!(
            ok[1].email,
            "nav-pub-sample-litigation@neon-law-stg.iam.gserviceaccount.com"
        );

        let mixed = resolve_publishers(
            "neon-law-stg",
            &[
                "sample-estate".to_string(),
                "a".repeat(PUBLISHER_CODE_MAX_LEN + 1),
            ],
        );
        assert!(
            matches!(mixed, Err(SetupError::PublisherCodeRefused { .. })),
            "one unfittable code must refuse the whole list",
        );
    }

    #[test]
    fn condition_pins_the_org_and_main_only() {
        let condition = wif_attribute_condition("neon-law");
        assert!(condition.contains("assertion.repository_owner == 'neon-law'"));
        assert!(condition.contains("assertion.ref == 'refs/heads/main'"));
    }

    #[test]
    fn principal_set_pins_one_repository() {
        let principal = wif_principal_set("123456789012", "neon-law", "acme");
        assert!(principal.starts_with("principalSet://iam.googleapis.com/projects/123456789012/"));
        assert!(principal
            .ends_with("workloadIdentityPools/app-publisher/attribute.repository/neon-law/acme"));
    }

    #[test]
    fn provider_resource_names_the_ghe_oidc_provider() {
        assert_eq!(
            wif_provider_resource("123456789012"),
            "projects/123456789012/locations/global/workloadIdentityPools/app-publisher/providers/ghe-oidc"
        );
    }

    fn joined_calls(client: &GcpClient) -> String {
        client
            .recorded_calls()
            .iter()
            .map(|c| format!("{} {}", c.url, c.body.as_deref().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn dry_run_records_the_full_publisher_provisioning() {
        let client = offline_dry_run_client();
        ensure(
            &client,
            "neon-law-stg",
            "neon-law",
            &["acme".to_string()],
            "neon-law-stg-applications",
        )
        .await
        .unwrap();
        let calls = client.recorded_calls();
        // SA create + custom role create + bucket IAM get + bucket IAM put +
        // WIF pool + WIF provider + impersonation get + impersonation set = 8
        // (project number is short-circuited in dry-run, so no CRM lookup).
        //
        // The custom role is its own call because defining a role and binding it
        // are separate operations: the definition is project-level and grants
        // nothing until the conditioned binding on the bucket references it.
        assert_eq!(calls.len(), 8, "unexpected dry-run calls: {calls:?}");
        let joined = joined_calls(&client);
        assert!(joined.contains("nav-pub-acme"));
        assert!(joined.contains("workloadIdentityPools?workloadIdentityPoolId=app-publisher"));
        assert!(joined.contains("providers?workloadIdentityPoolProviderId=ghe-oidc"));
        assert!(joined.contains(GITHUB_OIDC_ISSUER));
        assert!(joined.contains("assertion.repository_owner == 'neon-law'"));
        // The publisher holds the custom create-and-update role, never a
        // predefined role that carries delete.
        assert!(joined.contains(PUBLISHER_ROLE_ID));
        assert!(!joined.contains("roles/storage.objectAdmin"));
        assert!(!joined.contains("roles/storage.objectUser"));
        // And never `objects.list`, which cannot be prefix-scoped.
        assert!(!joined.contains("storage.objects.list"));
    }

    /// Two Projects get two accounts, two prefixes, and two impersonations —
    /// and still exactly one custom role, one pool and one provider.
    ///
    /// The count is the assertion that matters. Eight calls for one Project and
    /// thirteen for two means the five per-Project calls doubled while the three
    /// deployment-level POSTs did not, which is what hoisting them out of the
    /// loop buys. And each Project's condition names only its own prefix, so
    /// neither publisher can write into the other's portal.
    #[tokio::test]
    async fn two_projects_get_two_publishers_and_one_shared_pool() {
        let client = offline_dry_run_client();
        ensure(
            &client,
            "neon-law-stg",
            "neon-law",
            &["sample-estate".to_string(), "sample-litigation".to_string()],
            "neon-law-stg-applications",
        )
        .await
        .unwrap();
        let calls = client.recorded_calls();
        // 3 deployment-level (role, pool, provider) + 2 x 5 per Project (SA
        // create, bucket IAM get, bucket IAM put, impersonation get,
        // impersonation set) = 3 + 10 = 13.
        assert_eq!(calls.len(), 13, "unexpected dry-run calls: {calls:?}");
        let joined = joined_calls(&client);

        assert!(joined.contains("nav-pub-sample-estate"));
        assert!(joined.contains("nav-pub-sample-litigation"));
        // One role, one pool, one provider for the whole deployment.
        assert_eq!(joined.matches("/roles?roleId=").count(), 1);
        assert_eq!(
            joined
                .matches("workloadIdentityPools?workloadIdentityPoolId=app-publisher")
                .count(),
            1,
        );
        assert_eq!(
            joined
                .matches("providers?workloadIdentityPoolProviderId=ghe-oidc")
                .count(),
            1,
        );
        // Each impersonation is pinned to its own repository, so one Project's
        // CI cannot mint the other Project's publisher token.
        assert!(joined.contains("attribute.repository/neon-law/sample-estate"));
        assert!(joined.contains("attribute.repository/neon-law/sample-litigation"));
    }

    /// A code too long to own an account id refuses before the first request.
    ///
    /// Not merely "the run fails": the recording client must have seen nothing
    /// at all. Checking each code as the loop reached it would leave the
    /// Projects ahead of the bad one provisioned and the rest not, and a partial
    /// IAM apply is the state an operator has to unpick by hand.
    #[tokio::test]
    async fn a_too_long_code_refuses_before_any_call_is_made() {
        let client = offline_dry_run_client();
        let err = ensure(
            &client,
            "neon-law-stg",
            "neon-law",
            &["sample-estate".to_string(), "a".repeat(23)],
            "neon-law-stg-applications",
        )
        .await
        .expect_err("a code that cannot own an account id must refuse the stage");
        assert!(
            matches!(err, SetupError::PublisherCodeRefused { .. }),
            "{err}",
        );
        assert!(
            client.recorded_calls().is_empty(),
            "nothing may be provisioned before the whole list validates: {:?}",
            client.recorded_calls(),
        );
    }

    const PUBLISHER: &str = "nav-pub-sample-litigation@proj.iam.gserviceaccount.com";
    const PUBLISHER_MEMBER: &str =
        "serviceAccount:nav-pub-sample-litigation@proj.iam.gserviceaccount.com";

    /// The condition names the prefix itself *and* everything beneath it.
    ///
    /// The equality clause is not redundant. gcloud probes the destination
    /// prefix as though it were an object before writing, and a condition
    /// carrying only `startsWith(".../portal/")` denies that probe — the publish
    /// then fails before uploading anything, with a `403` on the prefix path and
    /// no trailing slash.
    #[test]
    fn the_condition_covers_the_prefix_path_and_its_children_only() {
        let expression = publisher_condition_expression("proj-applications", "sample-litigation");
        assert!(expression.contains(
            "resource.name == \"projects/_/buckets/proj-applications/objects/\
             sample-litigation/portal\""
        ));
        assert!(expression.contains(
            "resource.name.startsWith(\"projects/_/buckets/proj-applications/objects/\
             sample-litigation/portal/\")"
        ));
        // Another Project's prefix is not named at all.
        assert!(!expression.contains("sample-estate"));
    }

    /// The custom role holds create and update, and neither delete nor list.
    #[test]
    fn the_custom_role_is_create_and_update_only() {
        assert!(PUBLISHER_PERMISSIONS.contains(&"storage.objects.create"));
        assert!(PUBLISHER_PERMISSIONS.contains(&"storage.objects.update"));
        assert!(
            !PUBLISHER_PERMISSIONS.contains(&"storage.objects.delete"),
            "the publish never deletes; granting delete would remove the one \
             property the never-delete upload order relies on",
        );
        assert!(
            !PUBLISHER_PERMISSIONS.contains(&"storage.objects.list"),
            "listing is evaluated against the bucket, so no object-name \
             condition can scope it, and it would leak every other Project's \
             object names",
        );
        assert_eq!(
            publisher_role_name("proj"),
            "projects/proj/roles/navigatorApplicationsPublisher",
        );
    }

    /// An empty policy gains one conditioned binding, and `version: 3` with it.
    ///
    /// Without `version: 3` a conditioned binding is rejected outright, so the
    /// two travel together or neither works.
    #[tokio::test]
    async fn the_conditioned_binding_is_added_to_an_empty_policy_at_version_three() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .and(query_param("optionsRequestedPolicyVersion", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .and(body_partial_json(json!({
                "version": 3,
                "bindings": [{
                    "role": "projects/proj/roles/navigatorApplicationsPublisher",
                    "members": [PUBLISHER_MEMBER],
                    "condition": {
                        "expression": publisher_condition_expression(
                            "proj-applications", "sample-litigation"),
                    },
                }],
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        ensure_publisher_grant(
            &client,
            "proj",
            "proj-applications",
            "sample-litigation",
            PUBLISHER,
        )
        .await
        .unwrap();
    }

    /// A converged policy is left alone — no PUT at all.
    ///
    /// `navigator ops gcp setup` is expected to be re-runnable, and a second run
    /// reporting no change is the observable form of that. No PUT mock is
    /// mounted, so a write fails the test rather than passing silently.
    #[tokio::test]
    async fn a_converged_policy_is_not_rewritten() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .and(query_param("optionsRequestedPolicyVersion", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "version": 3,
                "bindings": [{
                    "role": "projects/proj/roles/navigatorApplicationsPublisher",
                    "members": [PUBLISHER_MEMBER],
                    "condition": {
                        "expression": publisher_condition_expression(
                            "proj-applications", "sample-litigation"),
                    },
                }],
            })))
            .mount(&server)
            .await;
        // What GCS actually answers when the read does *not* pin version 3:
        // a version 1 policy with the condition dropped and the role mangled.
        // Mounted second, so it only serves a request the version-3 mock above
        // did not match. If the query parameter is ever dropped, this response
        // is what the merge sees — it recognises nothing, appends a duplicate
        // binding and writes, and the absent PUT mock fails the test. That is
        // the whole point: the parameter is guarded by consequence, not by an
        // assertion that can rot beside the code.
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "version": 1,
                "etag": "CAE=",
                "bindings": [{
                    "role": "projects/proj/roles/\
                             navigatorApplicationsPublisher_withcond_2b17cc25d2cd9e2c",
                    "members": [PUBLISHER_MEMBER],
                }],
            })))
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        ensure_publisher_grant(
            &client,
            "proj",
            "proj-applications",
            "sample-litigation",
            PUBLISHER,
        )
        .await
        .unwrap();
    }

    /// A soft-deleted custom role is refused, not read as "already exists".
    ///
    /// A custom role's id stays reserved for seven days after deletion, and
    /// `roles.create` answers 409 for the whole window while the role itself
    /// stays unusable. Reading that 409 as success would provision a publisher
    /// holding a binding to a role that grants nothing — a failure that first
    /// shows up in a Project repository's CI, far from its cause.
    #[tokio::test]
    async fn a_soft_deleted_custom_role_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/proj/roles"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/proj/roles/navigatorApplicationsPublisher",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "projects/proj/roles/navigatorApplicationsPublisher",
                "deleted": true,
            })))
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Iam, server.uri());
        let err = ensure_publisher_role(&client, "proj")
            .await
            .expect_err("a soft-deleted role must not be reported as converged");
        assert!(
            matches!(err, SetupError::AmbiguousLiveState { .. }),
            "{err}"
        );
        assert!(
            err.to_string().contains("undelete"),
            "the refusal must say how to recover: {err}",
        );
    }

    /// A live custom role still makes a 409 a success.
    ///
    /// The soft-delete check must not turn ordinary idempotency into an error —
    /// a re-run against an existing, usable role is the common case.
    #[tokio::test]
    async fn an_existing_custom_role_is_converged() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/projects/proj/roles"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/v1/projects/proj/roles/navigatorApplicationsPublisher",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "projects/proj/roles/navigatorApplicationsPublisher",
                "includedPermissions": PUBLISHER_PERMISSIONS,
            })))
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Iam, server.uri());
        ensure_publisher_role(&client, "proj").await.unwrap();
    }

    /// A superseded wide grant is stripped, not left beside the narrow one.
    ///
    /// This is the reconcile case: production was hand-patched to unconditioned
    /// `objectAdmin` when create-only refused a republish. IAM is a union of
    /// grants, so adding the conditioned binding while leaving `objectAdmin` in
    /// place would narrow nothing at all. Another member of the same wide
    /// binding is kept — only the publisher is stripped out of it.
    #[tokio::test]
    async fn a_superseded_wide_grant_is_stripped_from_the_publisher() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .and(query_param("optionsRequestedPolicyVersion", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "bindings": [{
                    "role": "roles/storage.objectAdmin",
                    "members": [PUBLISHER_MEMBER, "serviceAccount:proj-web@proj.iam.gserviceaccount.com"],
                }],
            })))
            .mount(&server)
            .await;
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = std::sync::Arc::clone(&captured);
        Mock::given(method("PUT"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .respond_with(move |req: &wiremock::Request| {
                sink.lock()
                    .expect("lock")
                    .push(String::from_utf8_lossy(&req.body).to_string());
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        ensure_publisher_grant(
            &client,
            "proj",
            "proj-applications",
            "sample-litigation",
            PUBLISHER,
        )
        .await
        .unwrap();

        let body = captured.lock().expect("lock").join("");
        let policy: Value = serde_json::from_str(&body).expect("the written policy parses");
        let bindings = policy["bindings"].as_array().expect("bindings written");

        let admin = bindings
            .iter()
            .find(|b| b["role"] == "roles/storage.objectAdmin")
            .expect("the other member's binding survives");
        let members: Vec<&str> = admin["members"]
            .as_array()
            .expect("members")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            !members.contains(&PUBLISHER_MEMBER),
            "the publisher must be stripped from the wide grant, got {members:?}",
        );
        assert!(
            members.contains(&"serviceAccount:proj-web@proj.iam.gserviceaccount.com"),
            "an unrelated member of the same binding must be preserved, got {members:?}",
        );

        assert!(
            bindings.iter().any(|b| {
                b["role"] == "projects/proj/roles/navigatorApplicationsPublisher"
                    && b["condition"]["expression"].is_string()
            }),
            "the conditioned narrow binding must be present: {bindings:?}",
        );
        assert_eq!(policy["version"], json!(3));
    }

    /// A publisher already bound to a *different* prefix is refused, not
    /// silently repointed.
    ///
    /// Two situations produce this policy and the bucket cannot tell them
    /// apart: a repository rename, where overwriting is right, and a *second*
    /// Project being provisioned against the same publisher, where overwriting
    /// revokes the first Project's publish and reports success. The publisher
    /// account is derived from the GCP project id alone, so every Project in a
    /// deployment shares it and the second case is the one a rollout hits.
    ///
    /// Refusing costs an operator one manual step after a rename. Guessing
    /// costs a live Project its ability to publish, silently.
    #[tokio::test]
    async fn a_publisher_bound_to_another_prefix_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-applications/iam"))
            .and(query_param("optionsRequestedPolicyVersion", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "version": 3,
                "bindings": [{
                    "role": "projects/proj/roles/navigatorApplicationsPublisher",
                    "members": [PUBLISHER_MEMBER],
                    "condition": {
                        "expression": publisher_condition_expression(
                            "proj-applications", "sample-estate"),
                    },
                }],
            })))
            .mount(&server)
            .await;
        // No PUT mock: a write is a test failure, not a silent revocation.
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri());
        let err = ensure_publisher_grant(
            &client,
            "proj",
            "proj-applications",
            "sample-litigation",
            PUBLISHER,
        )
        .await
        .expect_err("a publisher already bound to another prefix must be refused");

        let message = err.to_string();
        assert!(
            matches!(err, SetupError::AmbiguousLiveState { .. }),
            "the refusal must be its own error, not a transport failure: {message}",
        );
        // Both prefixes are named, so the operator can tell a rename from a
        // second Project without reading the bucket policy by hand.
        assert!(
            message.contains("sample-estate") && message.contains("sample-litigation"),
            "the refusal must name both prefixes: {message}",
        );
    }

    /// The bucket policy is read at version 3, and that is load-bearing.
    ///
    /// Asked for version 1 — the default — IAM returns conditional bindings with
    /// the condition stripped and the role mangled to `<role>_withcond_<hash>`.
    /// This module would not recognise its own binding, would append a duplicate
    /// beside the mangled one, and would PUT a policy naming a role that does
    /// not exist. Every run after the first would write, and be rejected.
    #[test]
    fn the_bucket_policy_is_read_at_version_three() {
        assert_eq!(
            bucket_iam_path("proj-applications"),
            "/storage/v1/b/proj-applications/iam?optionsRequestedPolicyVersion=3",
        );
    }

    #[tokio::test]
    async fn wif_provider_posts_tenant_issuer_and_owner_pinned_condition() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/v1/projects/p/locations/global/workloadIdentityPools/app-publisher/providers",
            ))
            .and(query_param("workloadIdentityPoolProviderId", "ghe-oidc"))
            .and(body_partial_json(json!({
                "oidc": { "issuerUri": "https://token.actions.githubusercontent.com" },
                "attributeCondition":
                    "assertion.repository_owner == 'neon-law' && assertion.ref == 'refs/heads/main'"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "done": true })))
            .expect(1)
            .mount(&server)
            .await;
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Iam, server.uri());
        ensure_wif_provider(&client, "p", "neon-law").await.unwrap();
    }
}
