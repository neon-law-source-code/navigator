//! `devx ship` — one-shot "roll one configured deployment onto a tagged image".
//!
//! This is the deterministic, in-binary form of the production rollout
//! path documented in `docs/cloud-operations.md`. That public doc keeps
//! the prose rationale ("why each step, in this order"); this module is
//! the executable that runs the steps so an operator types one command
//! instead of pasting several shell blocks.
//!
//! **CI builds and publishes; ship only rolls.** The daily
//! `deploy.yml` tag flow builds service images and publishes them to
//! GHCR under an immutable release tag; this module never
//! builds or pushes an image — it pins the running cluster to an
//! already-published tag.
//!
//! Two flows, matching the documented rollout path:
//!
//! - **Roll** (default): take an immutable release tag to deploy (required
//!   `--tag`) → confirm every image is published at that tag → render the
//!   embedded GKE tree with the deployer's `NAVIGATOR_*` values **and the
//!   tag** → confirm the prod Secret satisfies the new binary's boot
//!   invariants against that rendered tree → `kubectl apply -k` it → wait
//!   out the rollouts → re-register the worker with Restate. Every service
//!   deployment lands on the **same** tag — never a version skew.
//!
//!   The order is the safety property, not an implementation detail.
//!   Everything before the apply is local or read-only, so an unsatisfied
//!   boot invariant aborts a ship that has changed nothing. And the apply
//!   carries the real tag rather than a placeholder a later `set image`
//!   corrects: `workflows-service` (`maxSurge: 0`) deletes the running pod
//!   before the replacement is ready, so a first write on an unpullable tag
//!   takes that tier down for however long the gap lasts. One write, one
//!   `ReplicaSet`.
//! - **No-rebuild restart** (`--restart-only`): after a Secret value
//!   was rotated, `kubectl rollout restart` service deployments that
//!   `envFrom` the Secret, so the pods re-read it (pods cache `envFrom` at
//!   start and never reload).
//!
//! Everything that varies per deployment comes from the repository's
//! `deployments/<name>/config.toml`, selected by the required
//! `--deployment` flag — there is no literal project ID, region, domain,
//! or registry prefix in this file, and no coordinate is ever read from
//! the process environment. A stale shell can therefore never select the
//! wrong deployment: the flag is the whole safety property. See
//! [`ShipConfig::from_deployment`].
//!
//! ## What this does NOT do
//!
//! - It never builds or pushes images. CI owns that; ship rolls a
//!   tag CI already published. The public GitHub `YY.M.D` tag is the
//!   source restore point, so there is no git-bundle archive step.
//! - It never auto-patches a prod Secret. The invariant check *aborts*
//!   with the exact `kubectl patch` to run when a required key is
//!   missing — generating and writing a prod secret silently is a
//!   judgment call left to the operator.
//!
//! ## Testing
//!
//! The shell-out orchestration needs a real cluster, so it isn't
//! unit-tested. The pure pieces — env-driven config, the registry
//! image-URL formulas, the required-key parser, the missing-key diff, and the
//! embedded-manifest render (zero placeholders after substitution; a
//! missing substitution var fails by name) — are
//! covered by the `tests` module below.

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, ensure, Context, Result};
use include_dir::{include_dir, Dir};
use tempfile::TempDir;

use portal::chatwoot::NAVIGATOR_CHATWOOT_WEBSITE_TOKEN;
use store::NAVIGATOR_SIMULATED_MATTERS;

use super::registry;
use super::{require_auth, require_tools, run};

/// In-cluster Deployment + container names. These are workspace
/// conventions (the GKE overlay names them); they are not per-deploy
/// configuration, so they stay as constants rather than env vars.
const WEB_DEPLOYMENT: &str = "navigator-web";
/// The only publishable web images. A deployment chooses its immutable brand
/// image in its `config.toml`; no runtime flag can change the public face.
const BRAND_IMAGES: &[&str] = &["neon-server", "neon-server"];
const WORKFLOWS_DEPLOYMENT: &str = "workflows-service";

/// The GKE manifest tree, embedded at compile time. `ship` renders it —
/// substituting the deployer's `NAVIGATOR_*` values for the placeholder
/// tokens — into a throwaway temp dir at ship time, then `kubectl apply
/// -k`s it. Embedding the whole *directory* (not a per-file `include_str!`
/// list) is what keeps the manifests out of the operator's hands — only the
/// `deployments/` tree of the repository checkout is read at ship time — and
/// means a manifest added to the tree later is bundled automatically. The kustomize
/// root at `kustomization.yaml` references both `../exports` and
/// `../../../../k8s/base`, so their embedded trees are extracted alongside it
/// at the same relative offsets. This is the CLI-generated form of what the
/// deployer-private overlay used to hand-carry (see `docs/gke-prod.md`).
static GKE_MANIFESTS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../examples/deploy/k8s/gke");

/// Scheduled-job manifests referenced by the GKE overlay through `../exports`.
/// They must travel with the embedded GKE tree: `ops ship` renders into a
/// temporary directory and cannot fall back to the operator's checkout.
static EXPORTS_MANIFESTS: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../examples/deploy/k8s/exports");

/// The shared platform base (namespace and application workloads) the GKE kustomization pulls
/// in via `../../../../k8s/base`. Embedded and extracted next to
/// [`GKE_MANIFESTS`] so the relative `resources:` reference resolves inside
/// the temp dir.
static K8S_BASE_MANIFESTS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../k8s/base");

/// The shared Kustomize components, embedded and extracted next to
/// [`K8S_BASE_MANIFESTS`] so [`PRIVATE_MODE_COMPONENT`] resolves. Only
/// referenced when private mode is on, but embedded unconditionally: a
/// release binary has no workspace checkout to read them from later, and
/// the whole directory costs a few kilobytes.
static K8S_COMPONENT_MANIFESTS: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../k8s/components");

/// The kustomize root inside the rendered temp dir — the path `kubectl -k`
/// targets. Mirrors the manifests' own repo-relative layout so the base's
/// `../../../../k8s/base` reference resolves.
const GKE_KUSTOMIZE_SUBPATH: &str = "examples/deploy/k8s/gke";
const EXPORTS_KUSTOMIZE_SUBPATH: &str = "examples/deploy/k8s/exports";

/// Boot-required keys that ship as inline Deployment env / the
/// `navigator-otel-env` `ConfigMap` rather than the projected Secret, so the
/// `SecretProviderClass` is not expected to declare them and no deployment's
/// `deployments/<name>/` tree needs to carry them. Everything else in
/// `store::deployment::WEB_REQUIREMENTS` rides the Secret rail.
pub(super) const INLINE_ENV_WEB_KEYS: &[&str] = &[
    "NAVIGATOR_CLAMD_ADDR",          // inline env → the ClamAV service
    "NAVIGATOR_STORAGE_BACKEND",     // inline env
    "NAVIGATOR_APPLICATIONS_BUCKET", // inline env → the applications bucket coordinate
    "NAVIGATOR_EMAIL_BACKEND",       // inline env
    "GOOGLE_OAUTH_CLIENT_IDS",       // inline env allowlist
];

/// The Secret keys a `SecretProviderClass` actually projects. A
/// `secretObjects[0].data` entry counts only when its `objectName` names a
/// `path` mounted by `parameters.secrets`: CSI writes each mounted `path` as a
/// file and projects it under the mapped `key`, so a `data` entry whose
/// `objectName` has no source `path` produces nothing. Reading both blocks
/// (not just the `key` list) is what catches drift between them — a `key` left
/// in `secretObjects` after its source entry is removed or renamed no longer
/// counts as projected. Pure, so that drift case is unit-testable.
pub(super) fn spc_projected_keys(spc: &str) -> BTreeSet<String> {
    let doc: serde_json::Value = serde_yaml::from_str(spc).expect("parse SecretProviderClass yaml");
    // `parameters.secrets` is a YAML block scalar (a string) listing each
    // Secret Manager object and the in-volume `path` it lands at.
    let secrets_block = doc
        .pointer("/spec/parameters/secrets")
        .and_then(serde_json::Value::as_str)
        .expect("SecretProviderClass has spec.parameters.secrets");
    let sourced_paths: BTreeSet<String> = serde_yaml::from_str::<serde_json::Value>(secrets_block)
        .expect("parse parameters.secrets block")
        .as_array()
        .expect("parameters.secrets is a list")
        .iter()
        .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
        .map(ToString::to_string)
        .collect();
    doc.pointer("/spec/secretObjects/0/data")
        .and_then(serde_json::Value::as_array)
        .expect("SecretProviderClass has spec.secretObjects[0].data")
        .iter()
        .filter_map(|entry| {
            let object_name = entry
                .get("objectName")
                .and_then(serde_json::Value::as_str)?;
            let key = entry.get("key").and_then(serde_json::Value::as_str)?;
            sourced_paths.contains(object_name).then(|| key.to_string())
        })
        .collect()
}

/// The Secret keys the shipped `SecretProviderClass` projects into the
/// deployment's `*-web-secrets` Secret — read from the manifest itself, so
/// neither the ship guard below nor `ops secrets apply` can drift from what a
/// live Secret Manager CSI sync would actually create. The object names are
/// deployment-independent; only the project prefix is substituted at render.
pub(super) fn secret_provider_class_keys() -> BTreeSet<String> {
    let spc = GKE_MANIFESTS
        .get_file("secrets/secret-provider-class.yaml")
        .and_then(include_dir::File::contents_utf8)
        .expect("embedded SecretProviderClass manifest");
    spc_projected_keys(spc)
}

/// Where [`K8S_BASE_MANIFESTS`] is extracted, relative to the temp-dir root.
const K8S_BASE_SUBPATH: &str = "k8s/base";

/// Where [`K8S_COMPONENT_MANIFESTS`] is extracted, relative to the temp-dir
/// root — the repo-relative offset [`PRIVATE_MODE_COMPONENT`] assumes.
const K8S_COMPONENTS_SUBPATH: &str = "k8s/components";

/// The private-mode component, as referenced FROM the GKE kustomization
/// root. Four levels up out of `examples/deploy/k8s/gke`, matching how the
/// same file already reaches `../../../../k8s/base`.
const PRIVATE_MODE_COMPONENT: &str = "../../../../k8s/components/private-mode";

/// One placeholder → real-value substitution the render applies to every
/// embedded manifest file. `token` is the literal string in the placeholder
/// base (`YOUR_PROJECT_ID`, `your-domain.example`, …); `value` is the
/// deployer's real value resolved from its `env` var.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Substitution {
    token: &'static str,
    env: &'static str,
    value: String,
}

/// The substitution table `ship` resolves from the selected deployment's
/// `deployments/<name>/config.toml` before rendering. Every entry is
/// REQUIRED — a missing or blank coordinate bails naming both the
/// placeholder and the key, so a half-substituted manifest never reaches
/// the cluster (mirrors [`require_asset_base_url`]). These are exactly the
/// values the deployer-private overlay used to substitute (`docs/gke-prod.md`
/// §"The private overlay"): the GCP project (buckets / GSA), the primary
/// domain (www / workflows / HD / redirect URI / public
/// base URL), the required browser OAuth client ID, and the optional Gemini
/// Enterprise data-store client ID.
///
/// Takes a getter closure over the deployment's coordinates, exactly like
/// `portal::config::enforce_deployment_invariants`, so the failure message
/// is unit-tested against an in-memory map.
fn resolve_substitutions_for_deployment<F>(
    deployment: &str,
    tag: &str,
    get: F,
) -> Result<Vec<Substitution>>
where
    F: Fn(&str) -> Option<String>,
{
    const TABLE: &[(&str, &str)] = &[
        ("YOUR_PROJECT_ID", "NAVIGATOR_GCP_PROJECT_ID"),
        // NOTE: the registry path in every `image:` line renders from
        // IMAGES_PROJECT_TOKEN below, NOT from this one. `YOUR_PROJECT_ID`
        // names the project that holds the buckets and the GSA — which is not
        // where the images live.
        ("NAVIGATOR_WEB_IMAGE", "NAVIGATOR_WEB_IMAGE"),
        ("NAVIGATOR_PUBLIC_HOST", "NAVIGATOR_PUBLIC_HOST"),
        ("NAVIGATOR_WORKFLOWS_HOST", "NAVIGATOR_WORKFLOWS_HOST"),
        ("YOUR_DOCUMENTS_BUCKET", "NAVIGATOR_DOCUMENTS_BUCKET"),
        ("YOUR_APPLICATIONS_BUCKET", "NAVIGATOR_APPLICATIONS_BUCKET"),
        ("YOUR_ASSETS_BUCKET", "NAVIGATOR_ASSETS_BUCKET"),
        // The token is `YOUR_ASSET_BASE_URL`, not the key itself: the
        // manifest carries the key as an env *name* on the line above the
        // value, and substitution is a plain string replace, so a token
        // spelled `NAVIGATOR_ASSET_BASE_URL` would rewrite the name too.
        ("YOUR_ASSET_BASE_URL", "NAVIGATOR_ASSET_BASE_URL"),
        ("YOUR_EXPORTS_BUCKET", "NAVIGATOR_EXPORTS_BUCKET"),
        ("NAVIGATOR_GATEWAY_IP_NAME", "NAVIGATOR_GATEWAY_IP_NAME"),
        (
            "NAVIGATOR_GCP_SERVICE_ACCOUNT_ID",
            "NAVIGATOR_GCP_SERVICE_ACCOUNT_ID",
        ),
        ("navigator-web-secrets", "NAVIGATOR_WEB_SECRET_NAME"),
        ("YOUR_GOOGLE_OAUTH_REQUIRED_HD", "GOOGLE_OAUTH_REQUIRED_HD"),
        // The GCP region, for the resources that still live in GCP (GKE,
        // buckets, KMS). No longer part of an image reference: those come from
        // GHCR, which has no region.
        ("YOUR_GCP_REGION", "NAVIGATOR_GCP_LOCATION"),
        (
            "YOUR_OAUTH_CLIENT_ID_BROWSER",
            "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
        ),
    ];
    let mut substitutions = base_substitutions(deployment, tag, &get)?;
    substitutions.extend(
        TABLE
            .iter()
            .map(|&(token, env)| required_substitution(deployment, token, env, &get))
            .collect::<Result<Vec<_>>>()?,
    );
    let browser_client_id = substitutions
        .iter()
        .find(|substitution| substitution.token == "YOUR_OAUTH_CLIENT_ID_BROWSER")
        .expect("the required browser OAuth substitution is in TABLE")
        .value
        .clone();
    let gemini_client_id = match non_empty_env("NAVIGATOR_OAUTH_CLIENT_ID_GEMINI", &get) {
        Some(value) => {
            validate_google_oauth_client_id("NAVIGATOR_OAUTH_CLIENT_ID_GEMINI", &value)?;
            value
        }
        // Gemini Enterprise supplies or selects its OAuth client while the
        // data store is registered. A website rollout must not depend on that
        // later connector step. Reusing the browser ID in the rendered
        // allowlist is a harmless set duplicate; `config.toml` omits the key
        // until the real, distinct Gemini client ID exists.
        None => browser_client_id,
    };
    substitutions.push(Substitution {
        token: "YOUR_OAUTH_CLIENT_ID_GEMINI",
        env: "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI",
        value: gemini_client_id,
    });
    // Optional, and absence means `false`. A deployment carrying sample
    // matters has to say so; every other deployment says nothing and gets the
    // production answer. Deliberately not in TABLE, whose entries all bail
    // when missing: making this required would let a line deleted from a
    // production `config.toml` block a production roll, and the safe reading
    // of a missing value here is simply "these matters are real". The value is
    // passed through verbatim so `store::config::sample_matters` does the
    // parsing in one place; a typo reaches the pod and fails the boot loudly
    // rather than being silently coerced here.
    substitutions.push(Substitution {
        token: "YOUR_SIMULATED_MATTERS",
        env: NAVIGATOR_SIMULATED_MATTERS,
        value: non_empty_env(NAVIGATOR_SIMULATED_MATTERS, &get)
            .unwrap_or_else(|| "false".to_string()),
    });
    // Optional for the same reason, and absence means no support-chat widget.
    // Out of TABLE because a deployment that names no Chatwoot inbox is the
    // normal case, not a misconfiguration: a required entry here would block
    // every roll that had not adopted the widget. The empty default is what
    // `portal::chatwoot` reads as "off", so an omitted key and an explicitly
    // blank one land on the same answer. The key is imported from the crate
    // that reads it, so a rename cannot leave the rendered manifest naming a
    // variable the binary no longer looks at.
    substitutions.push(Substitution {
        token: "YOUR_CHATWOOT_WEBSITE_TOKEN",
        env: NAVIGATOR_CHATWOOT_WEBSITE_TOKEN,
        value: non_empty_env(NAVIGATOR_CHATWOOT_WEBSITE_TOKEN, &get).unwrap_or_default(),
    });
    // Optional, same shape as Chatwoot above: Sign in with Microsoft is a
    // second provider next to Google, off by default. An empty
    // `OAUTH_MICROSOFT_CLIENT_ID` is exactly what
    // `portal::oauth::Provider::microsoft_from_env` reads as "no second
    // provider" — `Ok(None)`, no button, every existing deployment stays
    // byte-identical — so an omitted key and an explicitly blank one land on
    // the same answer. Out of TABLE for the same reason as Chatwoot: a
    // required entry here would block every roll that has not yet registered
    // an Entra app registration.
    substitutions.push(Substitution {
        token: "YOUR_OAUTH_MICROSOFT_CLIENT_ID",
        env: "OAUTH_MICROSOFT_CLIENT_ID",
        value: non_empty_env("OAUTH_MICROSOFT_CLIENT_ID", &get).unwrap_or_default(),
    });
    // Optional for the same reason. Blank is safe even though
    // `microsoft_from_env` treats a set client id with no tenant allowlist as
    // a boot-failing misconfiguration (`OAuthSetupError::MissingTenantAllowlist`):
    // that only matters once `OAUTH_MICROSOFT_CLIENT_ID` above is also
    // non-empty, and a deployment that sets one is expected to set both.
    substitutions.push(Substitution {
        token: "YOUR_OAUTH_MICROSOFT_ALLOWED_TENANTS",
        env: "OAUTH_MICROSOFT_ALLOWED_TENANTS",
        value: non_empty_env("OAUTH_MICROSOFT_ALLOWED_TENANTS", &get).unwrap_or_default(),
    });
    Ok(substitutions)
}

fn base_substitutions<F>(deployment: &str, tag: &str, get: &F) -> Result<Vec<Substitution>>
where
    F: Fn(&str) -> Option<String>,
{
    let namespace = required_coordinate(deployment, "NAVIGATOR_K8S_NAMESPACE", get)?;
    let image_registry = non_empty_env("NAVIGATOR_IMAGE_REGISTRY", get)
        .unwrap_or_else(|| registry::DEFAULT_REGISTRY.to_string());

    Ok(vec![
        Substitution {
            token: "kind: Namespace\nmetadata:\n  name: navigator",
            env: "NAVIGATOR_K8S_NAMESPACE",
            value: format!("kind: Namespace\nmetadata:\n  name: {namespace}"),
        },
        Substitution {
            token: "namespace: navigator",
            env: "NAVIGATOR_K8S_NAMESPACE",
            value: format!("namespace: {namespace}"),
        },
        Substitution {
            token: "navigator.svc.cluster.local",
            env: "NAVIGATOR_K8S_NAMESPACE",
            value: format!("{namespace}.svc.cluster.local"),
        },
        Substitution {
            token: IMAGE_REGISTRY_TOKEN,
            env: "NAVIGATOR_IMAGE_REGISTRY",
            value: image_registry,
        },
        // The release tag is a placeholder like any other. It renders here
        // — not via a later `kubectl set image` — so `apply -k` lands a
        // pullable image on the first write: `maxSurge: 0`
        // (workflows-service) deletes the running pod before the
        // replacement is ready, so an apply carrying an unpullable tag
        // takes that tier down for the whole gap until the tag is
        // corrected. `roll` validates the shape before we get here.
        Substitution {
            token: RELEASE_TAG_TOKEN,
            env: "--tag",
            value: tag.to_owned(),
        },
    ])
}

fn required_substitution<F>(
    deployment: &str,
    token: &'static str,
    env: &'static str,
    get: &F,
) -> Result<Substitution>
where
    F: Fn(&str) -> Option<String>,
{
    let value = non_empty_env(env, get).ok_or_else(|| {
        anyhow::anyhow!(
            "{env} must be set in deployments/{deployment}/config.toml for `navigator ops ship` \
             — it renders the `{token}` placeholder in the embedded GKE manifests. \
             Set it, then re-run \
             `navigator ops ship --deployment {deployment} --tag <YY.M.D>`."
        )
    })?;
    if token.starts_with("YOUR_OAUTH_CLIENT_ID") {
        validate_google_oauth_client_id(env, &value)?;
    }
    Ok(Substitution { token, env, value })
}

fn validate_google_oauth_client_id(env: &str, value: &str) -> Result<()> {
    if !value.ends_with(GOOGLE_OAUTH_CLIENT_ID_SUFFIX) {
        bail!(
            "{env} must be the FULL Google OAuth client id ending in \
             `{GOOGLE_OAUTH_CLIENT_ID_SUFFIX}` (got `{value}`) — a bare id renders an \
             `OAUTH_CLIENT_ID` Google will not match, breaking the login redirect."
        );
    }
    Ok(())
}

fn required_coordinate<F>(deployment: &str, env: &'static str, get: &F) -> Result<String>
where
    F: Fn(&str) -> Option<String>,
{
    non_empty_env(env, get)
        .ok_or_else(|| anyhow::anyhow!("{env} must be set in deployments/{deployment}/config.toml"))
}

fn non_empty_env<F>(env: &str, get: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(env).filter(|value| !value.trim().is_empty())
}

/// The suffix every Google OAuth client id carries. The render refuses a
/// bare id for the `YOUR_OAUTH_CLIENT_ID_*` placeholders so a missing suffix
/// fails at ship time, not at a user's first login.
const GOOGLE_OAUTH_CLIENT_ID_SUFFIX: &str = ".apps.googleusercontent.com";

/// The placeholder every `<registry>/navigator-*:` image in the
/// embedded tree carries. It is a substitution token, NOT an example of the
/// tag convention — the render replaces it with the `--tag` being rolled.
const RELEASE_TAG_TOKEN: &str = "YY.M.D";

/// The placeholder standing in for the registry namespace that HOLDS the
/// images — the `ghcr.io/<owner>` half of every image line.
///
/// Distinct from `YOUR_PROJECT_ID`, which renders the environment project
/// (buckets, GSA). The images do not live in a GCP project at all now, which
/// is the point: one token where there were three (region, hub project, repo
/// name), and no way for two of them to disagree.
const IMAGE_REGISTRY_TOKEN: &str = "YOUR_IMAGE_REGISTRY";

/// Apply every substitution to one manifest file's text. Plain string
/// replacement — the tokens are distinct and non-overlapping, so order is
/// irrelevant.
fn apply_substitutions(content: &str, subs: &[Substitution]) -> String {
    subs.iter().fold(content.to_string(), |acc, sub| {
        acc.replace(sub.token, &sub.value)
    })
}

/// Recursively write an embedded manifest `Dir` under `dest`, substituting
/// placeholders in every UTF-8 file (all our manifests are text; a
/// non-UTF-8 blob would be copied verbatim). Directory structure is
/// preserved so kustomize's relative `resources:`/`patches:` paths resolve.
fn render_embedded_dir(dir: &Dir, dest: &Path, subs: &[Substitution]) -> Result<()> {
    for file in dir.files() {
        let out = dest.join(file.path());
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create rendered manifest dir {}", parent.display()))?;
        }
        match file.contents_utf8() {
            Some(text) => fs::write(&out, apply_substitutions(text, subs)),
            None => fs::write(&out, file.contents()),
        }
        .with_context(|| format!("write rendered manifest {}", out.display()))?;
    }
    for child in dir.dirs() {
        render_embedded_dir(child, dest, subs)?;
    }
    Ok(())
}

/// Render the embedded GKE, exports, and base manifest trees into a fresh temp
/// dir with `subs` applied, returning the owned [`TempDir`] — dropping it
/// removes the rendered files, so the caller holds it only for the span of
/// the `kubectl` calls. The relative paths reproduce the repository layout
/// the kustomization references. The private-mode decision is passed in —
/// resolved from the deployment's own `config.toml`, never the process
/// environment — so the toggle is unit-tested without a `set_var` that would
/// leak into whatever else shares the process.
fn render_manifests_with(subs: &[Substitution], private_mode: bool) -> Result<TempDir> {
    let tmp = tempfile::Builder::new()
        .prefix("navigator-ship-manifests-")
        .tempdir()
        .context("create temp dir for rendered manifests")?;
    render_embedded_dir(
        &GKE_MANIFESTS,
        &tmp.path().join(GKE_KUSTOMIZE_SUBPATH),
        subs,
    )?;
    render_embedded_dir(
        &EXPORTS_MANIFESTS,
        &tmp.path().join(EXPORTS_KUSTOMIZE_SUBPATH),
        subs,
    )?;
    render_embedded_dir(
        &K8S_BASE_MANIFESTS,
        &tmp.path().join(K8S_BASE_SUBPATH),
        subs,
    )?;
    if private_mode {
        render_embedded_dir(
            &K8S_COMPONENT_MANIFESTS,
            &tmp.path().join(K8S_COMPONENTS_SUBPATH),
            subs,
        )?;
        let root = tmp
            .path()
            .join(GKE_KUSTOMIZE_SUBPATH)
            .join("kustomization.yaml");
        let kustomization = fs::read_to_string(&root)
            .with_context(|| format!("read rendered kustomization {}", root.display()))?;
        fs::write(&root, enable_private_mode(&kustomization, subs)?)
            .with_context(|| format!("write rendered kustomization {}", root.display()))?;
        eprintln!(
            "==> NAVIGATOR_PRIVATE_MODE is on — this ship puts the Pingora network + basic-auth gateway in \
             front of navigator-web (k8s/components/private-mode)"
        );
    }
    Ok(tmp)
}

/// Append the private-mode component to the rendered GKE kustomization.
///
/// Text append rather than a YAML round-trip: the kustomization is dense
/// with load-bearing comments (the commented-out CSI block, the `$patch:
/// delete` rationale) that a serialize/deserialize cycle would silently
/// drop. Bails rather than guesses if the file ever grows its own
/// `components:` key, since a second one is a duplicate mapping key and
/// kustomize would reject the tree only after the ship had already
/// started.
fn enable_private_mode(kustomization: &str, subs: &[Substitution]) -> Result<String> {
    if kustomization
        .lines()
        .any(|line| line.trim_start().starts_with("components:"))
    {
        bail!(
            "the GKE kustomization already declares `components:` — add \
             `{PRIVATE_MODE_COMPONENT}` to that list and delete this append, rather than \
             emitting a duplicate key"
        );
    }
    let gateway_image = apply_substitutions(
        &format!("{IMAGE_REGISTRY_TOKEN}/navigator-gateway:{RELEASE_TAG_TOKEN}"),
        subs,
    );
    let Some((gateway_name, gateway_tag)) = gateway_image.rsplit_once(':') else {
        bail!("gateway image ref {gateway_image:?} has no tag to pin");
    };
    Ok(format!(
        "{}\n\n# Added by `navigator ops ship` because NAVIGATOR_PRIVATE_MODE is on.\ncomponents:\n  - {PRIVATE_MODE_COMPONENT}\n\nimages:\n  - name: navigator-gateway\n    newName: {gateway_name}\n    newTag: \"{gateway_tag}\"\n",
        kustomization.trim_end()
    ))
}

/// The `SecretProviderClass`, relative to the rendered GKE kustomize root.
const SECRET_PROVIDER_CLASS: &str = "secrets/secret-provider-class.yaml";

/// Rewrite the rendered `SecretProviderClass` so it references exactly the
/// objects this deployment writes.
///
/// The object list is one embedded manifest shared by every deployment, and a
/// CSI mount fails the whole volume on a single object it cannot read. An entry
/// the shipping deployment will never write is therefore not a harmless extra
/// reference — it is a pod that never starts, and before that a ship that aborts
/// at [`ensure_projected_objects_resolve`]. One shared list cannot express an
/// object that is required in one deployment and forbidden in another (the
/// engineering webhook trio) or one belonging to an integration a deployment
/// declines outright (`DocuSign`), so the list is rendered per deployment from the
/// same `skipped` set `ops secrets apply` reports.
///
/// In place on the rendered copy, exactly like [`enable_private_mode`]: the
/// embedded tree is never touched, so the omission lasts for the span of one
/// ship and is visible in the `kubectl diff` that precedes the apply.
fn omit_unwritten_objects(gke_root: &Path, skipped: &BTreeSet<String>) -> Result<()> {
    if skipped.is_empty() {
        return Ok(());
    }
    let path = gke_root.join(SECRET_PROVIDER_CLASS);
    let spc = fs::read_to_string(&path)
        .with_context(|| format!("read rendered SecretProviderClass {}", path.display()))?;
    let filtered = without_projected_objects(&spc, skipped)?;
    fs::write(&path, filtered)
        .with_context(|| format!("write rendered SecretProviderClass {}", path.display()))?;
    eprintln!(
        "==> SecretProviderClass renders {} object(s) this deployment does not write: {}",
        skipped.len(),
        skipped.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

/// Drop every `parameters.secrets` and `secretObjects[0].data` entry naming an
/// object in `omitted`.
///
/// A line filter rather than a YAML round-trip, for the reason
/// [`enable_private_mode`] gives: this manifest carries load-bearing comments
/// (which objects were trimmed and why, which keys are boot invariants) that a
/// serialize/deserialize cycle would silently drop. Every entry is a head line
/// plus its one continuation, so the shape is asserted rather than assumed — an
/// entry that ever grows a third line fails here instead of rendering a
/// half-removed reference. The result is then re-parsed and checked, so the
/// guarantee is structural and not "the text looked right".
fn without_projected_objects(spc: &str, omitted: &BTreeSet<String>) -> Result<String> {
    let mut kept: Vec<&str> = Vec::new();
    let mut dropping = false;
    for line in spc.lines() {
        let trimmed = line.trim_start();
        if let Some(object) = entry_object_name(trimmed) {
            dropping = omitted.contains(&object);
        } else if dropping && !(trimmed.starts_with("path:") || trimmed.starts_with("key:")) {
            // The entry ended without our having seen its continuation, which
            // means the manifest no longer has the shape this filter removes.
            dropping = false;
        }
        if !dropping {
            kept.push(line);
        }
    }
    let mut filtered = kept.join("\n");
    if spc.ends_with('\n') {
        filtered.push('\n');
    }
    let surviving = projected_object_names(&filtered)?;
    let leaked: Vec<&String> = omitted.iter().filter(|o| surviving.contains(*o)).collect();
    ensure!(
        leaked.is_empty(),
        "the rendered SecretProviderClass still references {leaked:?} after omitting them; \
         refusing to ship a class whose mount would fail on an object this deployment does not write"
    );
    Ok(filtered)
}

/// The object name an entry head line names, for either of the two blocks.
fn entry_object_name(trimmed: &str) -> Option<String> {
    if let Some(rest) = trimmed.strip_prefix("- resourceName:") {
        let reference = rest.trim().trim_matches('"');
        let (_, tail) = reference.split_once("/secrets/")?;
        let (object, _) = tail.split_once("/versions/")?;
        return Some(object.to_string());
    }
    trimmed
        .strip_prefix("- objectName:")
        .map(|rest| rest.trim().trim_matches('"').to_string())
}

/// Every object name a `SecretProviderClass` document references, from both
/// blocks — the check side of [`without_projected_objects`], so a leftover
/// `secretObjects` entry is caught as loudly as a leftover mount.
fn projected_object_names(spc: &str) -> Result<BTreeSet<String>> {
    let doc: serde_json::Value =
        serde_yaml::from_str(spc).context("parse the filtered SecretProviderClass")?;
    let secrets_block = doc
        .pointer("/spec/parameters/secrets")
        .and_then(serde_json::Value::as_str)
        .context("the filtered SecretProviderClass has spec.parameters.secrets")?;
    let mut names: BTreeSet<String> = serde_yaml::from_str::<serde_json::Value>(secrets_block)
        .context("parse the filtered parameters.secrets block")?
        .as_array()
        .context("parameters.secrets is a list")?
        .iter()
        .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
        .map(ToString::to_string)
        .collect();
    if let Some(data) = doc
        .pointer("/spec/secretObjects/0/data")
        .and_then(serde_json::Value::as_array)
    {
        names.extend(
            data.iter()
                .filter_map(|entry| entry.get("objectName").and_then(serde_json::Value::as_str))
                .map(ToString::to_string),
        );
    }
    Ok(names)
}

/// The ordered `kubectl -k` verbs the reconcile runs against the rendered
/// dir. A live run diffs (surface drift) then applies; `--dry-run` stops
/// after the diff and never applies — the machine-checkable form of "shows
/// the diff with no apply".
fn reconcile_verbs(dry_run: bool) -> &'static [&'static str] {
    if dry_run {
        &["diff"]
    } else {
        &["diff", "apply"]
    }
}

/// Every per-deployment value `ship` reads, resolved once from the selected
/// `deployments/<name>/config.toml`. Required values bail when unset (fail
/// fast — never substitute a project-internal default). Optional values fall
/// back to a documented workspace default or to `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipConfig {
    /// The selected deployment's directory name under `deployments/`. Every
    /// error that tells the operator to fix a coordinate names this file.
    pub name: String,
    /// Typed deployment profile. `dev` is cloud staging; production is live.
    pub environment: store::DeploymentEnvironment,
    /// Target GCP project ID (`NAVIGATOR_GCP_PROJECT_ID`). Used to
    /// derive the `kubectl` context.
    pub project_id: String,
    /// Region the cluster lives in (`NAVIGATOR_GCP_LOCATION`). Used to
    /// derive the `kubectl` context.
    pub location: String,
    /// Cluster name (`NAVIGATOR_GKE_CLUSTER_NAME`). Used to derive the
    /// `kubectl` context.
    pub cluster: String,
    /// Public registry namespace the images live under — `ghcr.io/<owner>`,
    /// from `NAVIGATOR_IMAGE_REGISTRY` (default
    /// `ghcr.io/neon-law-source-code`). A fork overrides that one variable
    /// rather than a hard-coded value.
    pub registry: String,
    /// K8s namespace for the Deployments (`NAVIGATOR_K8S_NAMESPACE`,
    /// default `navigator`).
    pub namespace: String,
    /// Immutable brand image name (`NAVIGATOR_WEB_IMAGE`).
    pub web_image_name: String,
    /// Exact public hostname (`NAVIGATOR_PUBLIC_HOST`) used for smoke checks.
    pub public_host: String,
    /// Google service-account id bound to the runtime Kubernetes service
    /// accounts (`NAVIGATOR_GCP_SERVICE_ACCOUNT_ID`).
    pub google_service_account_id: String,
    /// Public hostname for the post-rollout smoke check — the resolved
    /// `brand.primary_domain` (Neon Law by default, or a custom bundle's).
    pub primary_domain: String,
    /// Name of the K8s Secret service deployments `envFrom`
    /// (`NAVIGATOR_WEB_SECRET_NAME`, default `navigator-web-secrets`).
    pub secret_name: String,
    /// Public worker URL Restate Cloud dials (`NAVIGATOR_WORKFLOWS_URL`).
    /// `None` → fall through to the `devx restate register` default.
    pub workflows_url: Option<String>,
    /// `kubectl` context to pin every prod call to. Override with
    /// `NAVIGATOR_GKE_CONTEXT`; otherwise the GKE convention
    /// `gke_<project>_<location>_<cluster>`.
    pub context: String,
}

impl ShipConfig {
    /// Resolve every value from a loaded `deployments/<name>/` tree — the
    /// only source; nothing here reads the process environment.
    pub fn from_deployment(deployment: &super::deployments::Deployment) -> Result<Self> {
        Self::from_lookup(&deployment.name, |key| {
            deployment.coordinates.get(key).cloned()
        })
    }

    /// The resolution behind [`ShipConfig::from_deployment`], split from the
    /// tree load so it is unit-tested against an in-memory coordinate map.
    /// Bails on the first missing required key with a message naming it and
    /// the `config.toml` it belongs in.
    fn from_lookup<F>(name: &str, get: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let environment = store::DeploymentEnvironment::from_lookup(&get)
            .context("parse NAVIGATOR_ENVIRONMENT")?;
        let project_id = required_coordinate(name, "NAVIGATOR_GCP_PROJECT_ID", &get)?;
        let location = required_coordinate(name, "NAVIGATOR_GCP_LOCATION", &get)?;
        let cluster = required_coordinate(name, "NAVIGATOR_GKE_CLUSTER_NAME", &get)?;
        let registry = images_registry(non_empty_env("NAVIGATOR_IMAGE_REGISTRY", &get).as_deref());
        let primary_domain = super::brand::primary_domain_with(&get)?;
        let namespace =
            non_empty_env("NAVIGATOR_K8S_NAMESPACE", &get).unwrap_or_else(|| "navigator".into());
        let web_image_name = required_coordinate(name, "NAVIGATOR_WEB_IMAGE", &get)?;
        if !BRAND_IMAGES.contains(&web_image_name.as_str()) {
            bail!(
                "NAVIGATOR_WEB_IMAGE must be one of {}; got `{web_image_name}`",
                BRAND_IMAGES.join(", ")
            );
        }
        let public_host = required_coordinate(name, "NAVIGATOR_PUBLIC_HOST", &get)?;
        let google_service_account_id = non_empty_env("NAVIGATOR_GCP_SERVICE_ACCOUNT_ID", &get)
            .unwrap_or_else(|| "navigator-web".into());
        let secret_name = non_empty_env("NAVIGATOR_WEB_SECRET_NAME", &get)
            .unwrap_or_else(|| "navigator-web-secrets".into());
        let workflows_url = non_empty_env("NAVIGATOR_WORKFLOWS_URL", &get);
        let context = non_empty_env("NAVIGATOR_GKE_CONTEXT", &get)
            .unwrap_or_else(|| derived_context(&project_id, &location, &cluster));
        Ok(Self {
            name: name.to_owned(),
            environment,
            project_id,
            location,
            cluster,
            registry,
            namespace,
            web_image_name,
            public_host,
            google_service_account_id,
            primary_domain,
            secret_name,
            workflows_url,
            context,
        })
    }

    /// Public registry namespace — `ghcr.io/<owner>`. CI publishes every
    /// image under this prefix; ship rolls the cluster onto them.
    #[must_use]
    pub fn registry(&self) -> String {
        self.registry.clone()
    }

    /// Published immutable brand image URL at the `YY.M.D` `tag`.
    #[must_use]
    pub fn web_image(&self, tag: &str) -> String {
        format!("{}/{}:{tag}", self.registry(), self.web_image_name)
    }

    /// Published `navigator-workflows-service` image URL at the
    /// `YY.M.D` `tag`.
    #[must_use]
    pub fn workflows_image(&self, tag: &str) -> String {
        format!("{}/navigator-workflows-service:{tag}", self.registry())
    }

    /// The public worker URL the 7d re-register targets, resolved the
    /// same way `devx restate register` resolves it: explicit
    /// `NAVIGATOR_WORKFLOWS_URL` first, otherwise derived from the resolved
    /// `brand.primary_domain` (`https://workflows.<domain>/`), never
    /// the bare placeholder when a domain is known. This is what the
    /// 2026-06-10 ship needed — it had a domain but no explicit URL and
    /// fell through to `workflows.example.com`, silently no-op'ing the
    /// register.
    #[must_use]
    pub fn workflows_url_resolved(&self) -> String {
        super::resolve_workflows_url(
            None,
            self.workflows_url.as_deref(),
            Some(&self.primary_domain),
        )
    }
}

/// The GKE context naming convention `gcloud container clusters
/// get-credentials` writes. Factored out so the formula is testable.
#[must_use]
fn derived_context(project_id: &str, location: &str, cluster: &str) -> String {
    format!("gke_{project_id}_{location}_{cluster}")
}

/// The registry prefix ship pulls from.
///
/// The images live in one public GHCR namespace, not in the project being
/// shipped into: CI pushes each image once and every deployment pulls the same
/// digest, which is what makes staging a proving ring rather than a different
/// build. Factored out so the fallback is testable without mutating process
/// environment.
#[must_use]
pub(super) fn images_registry(configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| registry::DEFAULT_REGISTRY.to_string(), str::to_string)
}

/// The public origin `web` serves blog/marketing images from — normally the
/// site's same-origin private-bucket proxy, e.g.
/// `https://www.example.test/assets`. `server/public/img/`
/// is gitignored and never baked into the image, so with this unset the
/// rolled binary resolves content images against an empty same-origin
/// `/public` and every hero 404s. A coordinate of the selected
/// `deployments/<name>/config.toml`.
const ASSET_BASE_URL_KEY: &str = "NAVIGATOR_ASSET_BASE_URL";
const ASSETS_BUCKET_KEY: &str = "NAVIGATOR_ASSETS_BUCKET";

/// Whether the asset origin is usably configured (present and not blank).
fn asset_base_url_present(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.trim().is_empty())
}

/// Refuse to ship until the public asset origin is configured. A missing
/// value means the selected deployment's `config.toml` is missing the key —
/// the fix is to set it there, not to work around it, so name it explicitly
/// and stop before any rollout rather than deploy a site whose every image
/// 404s.
fn require_asset_base_url(deployment: &super::deployments::Deployment) -> Result<()> {
    require_asset_base_url_value(
        &deployment.name,
        deployment
            .coordinates
            .get(ASSET_BASE_URL_KEY)
            .map(String::as_str),
    )
}

/// The decision behind [`require_asset_base_url`], split from the tree
/// load so the failure message is unit-tested against literal inputs.
fn require_asset_base_url_value(deployment: &str, value: Option<&str>) -> Result<()> {
    if !asset_base_url_present(value) {
        bail!(
            "{ASSET_BASE_URL_KEY} must be set in deployments/{deployment}/config.toml \
             before shipping — it is the public origin `web` serves blog/marketing images from, \
             normally `<NAV_BASE_URL>/assets`, backed by the private NAVIGATOR_ASSETS_BUCKET \
             through the deployment's GKE workload identity. Without it \
             the rolled site resolves images against an empty `/public` and every hero 404s. Set \
             it there, then run \
             `navigator ops ship --deployment {deployment} --tag <YY.M.D>`."
        );
    }
    Ok(())
}

fn verify_assets_bucket_value_with<F>(
    deployment: &str,
    value: Option<&str>,
    verify: F,
) -> Result<()>
where
    F: FnOnce(&str) -> Result<()>,
{
    let Some(bucket) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        bail!(
            "{ASSETS_BUCKET_KEY} must be set in deployments/{deployment}/config.toml before \
             shipping — the asset preflight checks that deployment's bucket directly"
        );
    };
    verify(bucket).with_context(|| {
        format!("verify public assets for deployment `{deployment}` in bucket `gs://{bucket}`")
    })
}

fn verify_assets_bucket(deployment: &super::deployments::Deployment) -> Result<()> {
    verify_assets_bucket_value_with(
        &deployment.name,
        deployment
            .coordinates
            .get(ASSETS_BUCKET_KEY)
            .map(String::as_str),
        crate::assets::verify_bundled_slide_assets_bucket,
    )
}

/// A boot requirement shared with web's deployment-invariant validator.
///
/// `any_of` holds the alternatives of a disjunctive invariant
/// (`"A or B + C must be set"`): the requirement is satisfied when any one
/// alternative's keys are ALL present in the Secret ∪ Deployment env. A
/// plain `"KEY must be set"` invariant is the one-alternative, one-key
/// case.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SecretRequirement {
    /// Alternatives (split on `" or "`), each a conjunction of keys
    /// (split on `" + "`) that must all be present together.
    pub any_of: Vec<Vec<String>>,
    /// Env var that gates a conditional requirement (`"… when TRIGGER
    /// is …"`); the requirement applies only when the trigger is itself
    /// configured. `None` → always required.
    pub trigger: Option<String>,
}

impl SecretRequirement {
    /// True when any one alternative's keys are all in `satisfied`.
    #[must_use]
    pub fn is_satisfied_by(&self, satisfied: &BTreeSet<String>) -> bool {
        self.any_of
            .iter()
            .any(|alt| alt.iter().all(|key| satisfied.contains(key)))
    }

    /// The invariant's own phrasing — `"A or B + C"` — for error text.
    #[must_use]
    pub fn describe(&self) -> String {
        self.any_of
            .iter()
            .map(|alt| alt.join(" + "))
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

fn shared_web_requirements(project_id: &str) -> Vec<SecretRequirement> {
    store::deployment::WEB_REQUIREMENTS
        .iter()
        .filter(|requirement| {
            requirement
                .project_id
                .is_none_or(|required| project_id == required)
        })
        .map(|requirement| SecretRequirement {
            any_of: requirement
                .any_of
                .iter()
                .map(|alternative| alternative.iter().map(ToString::to_string).collect())
                .collect(),
            trigger: requirement.trigger.map(ToString::to_string),
        })
        .collect()
}

/// Test-only parser for invariant-shaped requirement literals.
/// enforces, by scraping the `"<KEYS> must be set` string literals
/// straight from the invariant source. Reading the source (rather than
/// maintaining a duplicate list) means this never drifts from the
/// binary's actual boot requirements.
///
/// The scraper is line-based: everything from the literal's opening `"`
/// through `" must be set"` must sit on one source line. Two shapes
/// parse:
///
/// - a single key (`"SENDGRID_API_KEY must be set (otherwise …"`) — the
///   one-alternative, one-key requirement;
/// - a disjunction (`"A or B + C must be set …"`) — alternatives split
///   on `" or "`, conjunctions on `" + "`; the pod boots when any one
///   alternative is fully configured.
///
/// Some invariants are conditional: the binary only enforces them when
/// another env var is itself set — e.g. `"OIDC_AUDIENCE must be set when
/// OIDC_JWKS_URL is …"`. The invariant message names its own trigger
/// ("… when `TRIGGER` is …"), so the trigger is read from the same
/// literal, staying faithful to the "scrape the source, never maintain a
/// parallel list" philosophy. A requirement that appears both
/// conditionally and unconditionally resolves to unconditional (it is
/// always required). Sorted + de-duplicated.
#[must_use]
#[cfg(test)]
pub fn secret_requirements(config_src: &str) -> Vec<SecretRequirement> {
    const MARKER: &str = " must be set";
    let mut reqs: BTreeMap<Vec<Vec<String>>, Option<String>> = BTreeMap::new();
    for line in config_src.lines() {
        let Some(end) = line.find(MARKER) else {
            continue;
        };
        let Some(any_of) = requirement_chain(&line[..end]) else {
            continue;
        };
        let trigger = trigger_after(&line[end + MARKER.len()..]);
        let unconditional = trigger.is_none();
        reqs.entry(any_of)
            .and_modify(|existing| {
                if unconditional {
                    *existing = None;
                }
            })
            .or_insert(trigger);
    }
    reqs.into_iter()
        .map(|(any_of, trigger)| SecretRequirement { any_of, trigger })
        .collect()
}

/// Walk left from `" must be set"` across identifier runs chained by
/// `" or "` / `" + "`, requiring the chain to open a string literal —
/// the char before the leftmost identifier is a double-quote. That
/// filters out prose that happens to contain "… must be set". Returns
/// the alternatives (`" or "`) of conjunctions (`" + "`) in source
/// order, or `None` when the text before the marker is not such a chain.
#[cfg(test)]
fn requirement_chain(prefix: &str) -> Option<Vec<Vec<String>>> {
    const OR: &str = " or ";
    const AND: &str = " + ";
    fn is_ident(b: u8) -> bool {
        b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
    }
    let bytes = prefix.as_bytes();
    let mut alternatives: Vec<Vec<String>> = Vec::new();
    let mut conjunction: Vec<String> = Vec::new();
    let mut end = bytes.len();
    loop {
        let mut start = end;
        while start > 0 && is_ident(bytes[start - 1]) {
            start -= 1;
        }
        if start == end || start == 0 {
            // No identifier where the chain requires one, or the chain
            // runs to line start without ever opening a literal.
            return None;
        }
        conjunction.insert(0, prefix[start..end].to_string());
        let before = &prefix[..start];
        if bytes[start - 1] == b'"' {
            alternatives.insert(0, conjunction);
            return Some(alternatives);
        }
        if let Some(rest) = before.strip_suffix(AND) {
            end = rest.len();
        } else {
            let rest = before.strip_suffix(OR)?;
            alternatives.insert(0, std::mem::take(&mut conjunction));
            end = rest.len();
        }
    }
}

/// Read the trigger key out of the tail of an invariant message — the
/// `TRIGGER` in `" when TRIGGER is …"`. Returns `None` for an
/// unconditional invariant (whose tail starts with `" (otherwise …"`).
#[cfg(test)]
fn trigger_after(tail: &str) -> Option<String> {
    let rest = tail.trim_start().strip_prefix("when ")?.trim_start();
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    (!ident.is_empty()).then_some(ident)
}

/// Of the parsed requirements, those actually applying to *this*
/// environment: every unconditional requirement, plus each conditional
/// one whose trigger is itself satisfied (present in the Secret or a
/// Deployment env). A conditional requirement whose trigger is absent
/// does not apply — the binary's runtime invariant skips it too.
#[must_use]
pub fn effective_requirements(
    parsed: &[SecretRequirement],
    satisfied: &BTreeSet<String>,
) -> Vec<SecretRequirement> {
    parsed
        .iter()
        .filter(|req| req.trigger.as_ref().is_none_or(|t| satisfied.contains(t)))
        .cloned()
        .collect()
}

/// Requirements no alternative of which is satisfied by anything the
/// running pod can see. `satisfied` is the union of the Secret's
/// non-empty data keys and the Deployments' usably-declared env-var
/// names — a key present in either is fine (a plain env var, a
/// `secretKeyRef`, or `envFrom` all count). What remains will
/// crash-loop the new pod at boot.
#[must_use]
pub fn unsatisfied_requirements(
    required: &[SecretRequirement],
    satisfied: &BTreeSet<String>,
) -> Vec<SecretRequirement> {
    required
        .iter()
        .filter(|req| !req.is_satisfied_by(satisfied))
        .cloned()
        .collect()
}

fn missing_requirements_by_deployment(
    parsed: &[SecretRequirement],
    secret_keys: &BTreeSet<String>,
    deployment_envs: &[(String, BTreeSet<String>)],
) -> Vec<(String, Vec<SecretRequirement>)> {
    deployment_envs
        .iter()
        .filter_map(|(deployment, env_names)| {
            let satisfied: BTreeSet<String> = secret_keys.union(env_names).cloned().collect();
            // Drop conditional invariants whose trigger isn't configured
            // in this deployment — the binary's own runtime check skips
            // them too, so requiring them would be a false positive.
            let required = effective_requirements(parsed, &satisfied);
            let missing = unsatisfied_requirements(&required, &satisfied);
            (!missing.is_empty()).then(|| (deployment.clone(), missing))
        })
        .collect()
}

// ---------- orchestration (shell-out; not unit-tested) ----------

/// Options parsed from the `devx ship` flags.
#[allow(clippy::struct_excessive_bools)] // One independent switch per CLI flag.
#[derive(Debug, Clone, Default)]
pub struct ShipOpts {
    /// Deployment directory under `deployments/` to roll. Required and
    /// explicit — there is no environment fallback, because a fallback is
    /// what lets a stale shell silently ship the wrong deployment.
    pub deployment: String,
    /// The directory holding the `deployments/` tree, when it is not the
    /// workspace — a checkout that carries the tree and nothing else.
    /// `None` falls back to `NAVIGATOR_DEPLOYMENTS_DIR`, then to the
    /// discovered workspace root. See [`super::deployments::root`].
    pub deployments_dir: Option<PathBuf>,
    /// Print every command instead of running it.
    pub dry_run: bool,
    /// No-rebuild path: just `kubectl rollout restart` service
    /// deployments (Secret-value rotation), then exit.
    pub restart_only: bool,
    /// The NARROW lane, for automated deploys: move every navigator image
    /// to `--tag` and nothing else.
    ///
    /// It exists so CI can hold a credential that cannot do anything but
    /// bump a version. The full roll re-asserts IAM on the web GSA and
    /// applies the whole manifest tree, which needs `serviceAccountAdmin`
    /// and write on every manifest kind; this needs `patch`/`get` on
    /// `apps/deployments` and `batch/cronjobs` in one namespace.
    ///
    /// It REFUSES when `kubectl diff -k` reports drift, because the
    /// manifest changes it is skipping are exactly the ones it cannot
    /// apply — silently running the cheap lane against a changed tree is
    /// the failure this flag would otherwise introduce.
    pub image_only: bool,
    /// The immutable `YY.M.D` or `YY.M.D-hotfix.N` tag to
    /// roll onto. Required for a roll —
    /// `None` is rejected (we never guess the latest tag); only the
    /// `--restart-only` path, which changes no image, runs without it.
    pub tag: Option<String>,
    /// Withdraw the roll's authority to write the web GSA's self-signing
    /// binding: verify it, and refuse when it is absent instead of granting
    /// it. For an operator under a no-IAM-changes rule — reading the policy
    /// needs `getIamPolicy`, writing it needs `setIamPolicy`, and this flag
    /// keeps the roll inside the first. It declines the write, never the
    /// check; see [`ensure_web_signing_iam`].
    pub assert_signing_iam: bool,
}

fn restart_deployments() -> &'static [&'static str] {
    &[WEB_DEPLOYMENT, WORKFLOWS_DEPLOYMENT]
}

fn rollout_wait_deployments() -> &'static [&'static str] {
    restart_deployments()
}

/// The Deployments that run the `web` binary and so answer to its boot
/// invariants.
fn web_binary_deployments() -> &'static [&'static str] {
    &[WEB_DEPLOYMENT]
}

/// Entry point for `Cmd::Ship`. Loads the selected deployment from the
/// `deployments/` tree — coordinates only; the encrypted file contributes key
/// names, never a value, and no KMS call is made.
pub fn run_ship(opts: &ShipOpts) -> Result<()> {
    let root = super::deployments::root(opts.deployments_dir.as_deref())?;
    let deployment = super::deployments::Deployment::load(&root, &opts.deployment)?;
    let cfg = ShipConfig::from_deployment(&deployment)?;
    // Prod must know where public images are served from before any pod
    // rolls onto the new tag — enforced for both the roll and the
    // restart-only push, since both recreate pods that read it.
    require_asset_base_url(&deployment)?;
    let lane = ship_lane(opts);
    if lane_requires_asset_preflight(lane) {
        // Slide-media references are embedded in the standalone CLI, so this
        // checks the selected bucket before either rollout lane can mutate the
        // cluster. A restart changes no image or content version.
        verify_assets_bucket(&deployment)?;
    }
    match lane {
        ShipLane::ImageOnly => image_only(&cfg, opts, &root),
        ShipLane::RestartOnly => restart_only(&cfg, opts.dry_run),
        ShipLane::Roll => roll(&cfg, &deployment, opts),
    }
}

/// Which of the three lanes an `ops ship` invocation selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShipLane {
    /// `--image-only`: move every navigator image to `--tag`, nothing else.
    ImageOnly,
    /// `--restart-only`: recreate the pods so they re-read a rotated Secret.
    RestartOnly,
    /// The full roll.
    Roll,
}

/// Resolve the lane from the flags, in one place rather than as a fall-through
/// chain inside [`run_ship`], so the precedence is provable without a cluster:
/// `--image-only` is the narrowest lane and so wins over `--restart-only`, and
/// a bare `ops ship` is the full roll.
fn ship_lane(opts: &ShipOpts) -> ShipLane {
    if opts.image_only {
        ShipLane::ImageOnly
    } else if opts.restart_only {
        ShipLane::RestartOnly
    } else {
        ShipLane::Roll
    }
}

fn lane_requires_asset_preflight(lane: ShipLane) -> bool {
    matches!(lane, ShipLane::ImageOnly | ShipLane::Roll)
}

/// The no-rebuild push: restart service deployments so the pods re-read
/// a rotated Secret value. Pods cache `envFrom` at start and never
/// reload, so a rotation is invisible until the pod is recreated.
fn restart_only(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    require_tools(&["kubectl"])?;
    require_auth(&["gcloud"])?;
    restart_only_steps(cfg, dry_run)
}

/// The restart lane past its preflight. Split from [`restart_only`] so a
/// `--dry-run` test drives the whole sequence: every step below prints instead
/// of shelling out under dry-run, so the two probes above — which need
/// `kubectl` on PATH and an authenticated `gcloud` — are the only part of the
/// lane a unit test cannot execute.
fn restart_only_steps(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    verify_context(cfg, dry_run)?;
    eprintln!("==> no-rebuild push: rollout restart service deployments (Secret rotation)");
    let mut cmd = kubectl(cfg);
    cmd.arg("rollout").arg("restart");
    for deployment in restart_deployments() {
        cmd.arg(format!("deployment/{deployment}"));
    }
    exec(dry_run, &mut cmd)?;
    wait_rollouts(cfg, dry_run, "120s", rollout_wait_deployments())?;
    eprintln!(
        "==> restart complete. VERIFY on the third-party side (the pod will 2xx against a \
         valid-but-wrong key) — compare upstream stats before/after."
    );
    Ok(())
}

/// Move every navigator image to `tag` and change nothing else.
///
/// FOUR STEPS, NOT ONE. "Set the image and wait" is the obvious reading and
/// it is wrong twice over:
///
///   * The six trigger `CronJob`s carry their own image. Skip them and the
///     web pods run the new version while every scheduled job keeps running
///     the old one — a skew nothing reports, because `CronJob`s do not roll.
///   * Restate resolves a worker's handler list at registration. A new
///     `workflows-service` image that Restate has not rediscovered leaves a
///     stale list, and `reregister`'s own error says webhook submissions then
///     fail SILENTLY. That is a live wrong-behaviour bug, not a cosmetic gap.
///
/// What it deliberately does NOT do is the expensive half of a full roll: the
/// web GSA's self-signing IAM re-assert, and `apply -k` over the whole
/// rendered tree. Both are idempotent no-ops on a routine version bump, and
/// both are what make the full roll's credential large.
fn image_only(cfg: &ShipConfig, opts: &ShipOpts, root: &Path) -> Result<()> {
    // Arguments before the machine. A missing or malformed `--tag` is knowable
    // without kubectl installed, gcloud authenticated, or a reachable cluster,
    // and the scheduled caller that gets it wrong should read that rather than
    // a toolchain complaint.
    let tag = image_only_tag(opts.tag.as_deref())?;

    require_tools(&["kubectl"])?;
    require_auth(&["gcloud"])?;
    image_only_steps(cfg, tag, opts.dry_run, root)
}

/// The narrow lane past its preflight, split from [`image_only`] for the same
/// reason [`restart_only_steps`] is: under `--dry-run` every step below prints
/// its command instead of running it, so a unit test walks the whole four-step
/// sequence — context cross-check, drift refusal, the image writes, the
/// `CronJob` re-pin, the rollout wait, the Restate re-register — with no
/// cluster, no `kubectl`, and no `gcloud`.
fn image_only_steps(cfg: &ShipConfig, tag: &str, dry_run: bool, root: &Path) -> Result<()> {
    verify_context(cfg, dry_run)?;

    refuse_on_manifest_drift(cfg, tag, dry_run, root)?;

    eprintln!("==> image-only roll: every navigator image → {tag}");
    for (deployment, image) in image_only_image_writes(cfg, tag) {
        let mut cmd = kubectl(cfg);
        cmd.arg("set")
            .arg("image")
            .arg(format!("deployment/{deployment}"))
            .arg(format!("{deployment}={image}"));
        exec(dry_run, &mut cmd)?;
    }

    pin_cronjob_images(cfg, tag, dry_run)?;
    wait_rollouts(cfg, dry_run, "300s", rollout_wait_deployments())?;
    reregister(cfg, dry_run)?;

    eprintln!(
        "==> image-only roll complete: {tag} live in {}",
        cfg.project_id
    );
    Ok(())
}

/// The `(Deployment, image)` pairs the narrow lane writes, in order.
///
/// One `set image` per Deployment rather than one combined call: the container
/// name inside each differs from the other, so a single invocation would need
/// `container=image` pairs that only hold for one of them. Two writes, two
/// `ReplicaSets`, both pinned to the same tag — which is what a test asserts
/// here, because a skew between them is the defect this lane must not
/// introduce.
fn image_only_image_writes(cfg: &ShipConfig, tag: &str) -> [(&'static str, String); 2] {
    [
        (WEB_DEPLOYMENT, cfg.web_image(tag)),
        (WORKFLOWS_DEPLOYMENT, cfg.workflows_image(tag)),
    ]
}

/// The release tag the narrow lane rolls onto, or the reason it cannot.
///
/// Two refusals, not one. The lane's only job is to move images to a named
/// release, so an absent `--tag` leaves it with nothing to do — and it never
/// guesses the latest published tag, exactly as the full roll never does.
/// A present tag still has to be a real `YY.M.D` or
/// `YY.M.D-hotfix.N` release name: the
/// scheduled caller derives the tag from a clock, and a derivation that
/// produces `2026-08-17` or an empty string must fail here rather than as a
/// pod pulling an image that does not exist.
fn image_only_tag(tag: Option<&str>) -> Result<&str> {
    let Some(tag) = tag else {
        bail!("--image-only requires --tag: the lane exists to move images to a named release");
    };
    registry::validate_release_tag(tag)?;
    Ok(tag)
}

/// Abort the narrow lane when the rendered tree differs from the cluster.
///
/// `kubectl diff` exits 0 for no drift, 1 for drift, and >1 for a real error
/// (unreachable context, auth failure, kustomize build error). The narrow lane
/// treats 1 as fatal — the opposite of the full roll, which treats it as the
/// normal signal that there is something to apply.
fn refuse_on_manifest_drift(cfg: &ShipConfig, tag: &str, dry_run: bool, root: &Path) -> Result<()> {
    if dry_run {
        eprintln!("DRY-RUN: would refuse the image-only roll if `kubectl diff -k` reported drift");
        return Ok(());
    }
    // Coordinates come from the deployment's own `config.toml`, never the
    // process environment — the same rule the full roll follows, so a stale
    // shell cannot render one deployment's tree while pointed at another.
    let deployment = super::deployments::Deployment::load(root, &cfg.name)?;
    let coordinate = |key: &str| deployment.coordinates.get(key).cloned();
    let subs = resolve_substitutions_for_deployment(&cfg.name, tag, coordinate)?;
    let rendered = render_manifests_with(&subs, false)?;
    let target = rendered.path().join(GKE_KUSTOMIZE_SUBPATH);
    let status = kubectl_ctx(cfg)
        .arg("diff")
        .arg("-k")
        .arg(&target)
        .status()
        .with_context(|| format!("spawn kubectl diff -k {}", target.display()))?;
    drift_verdict(status.code())
}

/// What one `kubectl diff -k` exit code means to the narrow lane.
///
/// Split out from the shell-out because the mapping is the whole safety
/// property and the shell-out is not testable: `kubectl diff` exits 0 for no
/// drift, 1 for drift, and >1 for a real error (unreachable context, auth
/// failure, kustomize build error). Only 0 may proceed.
///
/// 1 is fatal here and normal in the full roll — that inversion is the reason
/// this lane can hold a small credential, so it is asserted rather than left
/// to a reading of the code. `None` (killed by a signal, so no code at all) is
/// an error too: it is the absence of an answer, not a "no drift".
fn drift_verdict(code: Option<i32>) -> Result<()> {
    match code {
        Some(0) => Ok(()),
        Some(1) => bail!(
            "the rendered manifests differ from the cluster, so this is not a version bump. \
             `--image-only` applies no manifest change by design — running it here would move \
             the images and silently leave the rest of the diff unapplied. Roll with a full \
             `ops ship` instead."
        ),
        Some(other) => {
            bail!("kubectl diff -k failed with exit {other} — the cluster was not reached")
        }
        None => bail!("kubectl diff -k was killed by a signal — the cluster was not reached"),
    }
}

/// Roll the cluster onto an already-published immutable release tag.
/// CI built and published the images; this only updates the cluster:
/// resolve the tag → confirm the Secret satisfies the new binary's boot
/// invariants → pin service deployments AND every trigger `CronJob` to
/// that tag → wait → re-register the worker with Restate → smoke-check.
/// No build, no push, no skew — every navigator image in sync at one tag.
fn roll(
    cfg: &ShipConfig,
    deployment: &super::deployments::Deployment,
    opts: &ShipOpts,
) -> Result<()> {
    require_tools(&["kubectl"])?;
    // Authenticated, not just installed: gcloud carries the GKE
    // credentials the pinned kubectl context resolves against.
    require_auth(&["gcloud"])?;
    let dry_run = opts.dry_run;

    // 1. Pre-flight — confirm the prod context resolves before any call.
    verify_context(cfg, dry_run)?;

    // 1b. The immutable release tag to roll — always an explicit `--tag`, so
    //     the operator names the exact published release rather than letting
    //     the roll guess. A malformed tag is knowable without reaching GCP at
    //     all, so it is settled before the IAM check below: otherwise a typo
    //     reports an IAM failure. Present service deployments get the SAME tag.
    let Some(tag) = opts.tag.as_deref() else {
        bail!(
            "`--tag <YY.M.D|YY.M.D-hotfix.N>` is required: name the published release to roll onto. \
             We never guess the latest tag; pass the tag from the deploy hand-off \
             (or use `--restart-only` to re-read a rotated Secret without changing the image)."
        );
    };
    registry::validate_release_tag(tag)?;
    let tag = tag.to_string();

    // 1c. Assert the web GSA's self-signing IAM. Independent of the image tag,
    //     so it belongs before any rollout: document downloads issue GCS
    //     signed URLs, which the pod can only mint if its GSA holds
    //     serviceAccountTokenCreator on itself. Verify-then-assert, so the
    //     steady state is a read — see `ensure_web_signing_iam`.
    ensure_web_signing_iam(cfg, dry_run, signing_iam_authority(opts))?;

    // 2. Name the images this tag resolves to.
    let web_remote = cfg.web_image(&tag);
    let workflows_remote = cfg.workflows_image(&tag);
    eprintln!(
        "==> rolling {} ({}) onto {tag}\n      {web_remote}\n      {workflows_remote}",
        cfg.project_id, cfg.context
    );

    // 2b. Fail fast if the images aren't actually published at this tag —
    //     applying a manifest that pins a missing tag wedges the workload in
    //     ImagePullBackOff. A partial CI publish (the web tag lands but the
    //     workflows-service publish leg fails — they run as a fail-fast:false
    //     matrix) would otherwise roll workflows-service onto a missing tag,
    //     so verify every image on every live run. This precedes the render
    //     because the render bakes `tag` into the manifests the reconcile
    //     applies. Skipped in dry-run; that mode still performs the
    //     read-only Kubernetes Secret preflight and manifest diff.
    let coordinate = |key: &str| deployment.coordinates.get(key).cloned();
    let private_mode = super::private_mode(coordinate("NAVIGATOR_PRIVATE_MODE").as_deref());
    if !dry_run {
        for image in [cfg.web_image_name.as_str(), "navigator-workflows-service"] {
            registry::ensure_tag_published(&cfg.registry, image, &tag)?;
        }
        if private_mode {
            registry::ensure_tag_published(&cfg.registry, "navigator-gateway", &tag)?;
        }
    }

    // 3. Render the embedded manifest tree with the deployment's NAVIGATOR_*
    //    coordinates and the tag being rolled. Pure local work — nothing
    //    reaches the cluster until step 5, which is what lets step 4 abort
    //    for free.
    let subs = resolve_substitutions_for_deployment(&deployment.name, &tag, coordinate)?;
    let rendered = render_manifests_with(&subs, private_mode)?;
    let target = rendered.path().join(GKE_KUSTOMIZE_SUBPATH);

    // 3b. Render the projected object list for THIS deployment: drop every
    //     entry it does not write. The list is one shared manifest and a CSI
    //     mount fails the whole volume on a single unreadable object, so a
    //     deployment that declines DocuSign cannot mount a class referencing
    //     DocuSign — and must not be given a placeholder credential to make
    //     the reference resolve, since `portal::signature` reaches its stub
    //     only through genuine absence. Same for an object scoped to another
    //     deployment, which this one is forbidden to hold at all.
    omit_unwritten_objects(
        &target,
        &super::deployments::skipped_projected_objects(deployment)?,
    )?;

    // 4. Confirm the prod Secret satisfies the new binary's invariants —
    //    BEFORE the reconcile mutates anything. An unsatisfied requirement
    //    aborts a ship that has touched nothing; running this after the
    //    apply strands the cluster on a half-rolled state (which is exactly
    //    how the 26.7.15 ship wedged the git tier).
    let manifests = kustomize_build(&target)?;
    ensure_secret_invariants(cfg, dry_run, &manifests)?;

    // 4b. …and confirm the *source* of that Secret is readable. Once the CSI
    //     resource is in the overlay the Secret is projected from Secret
    //     Manager, so a single object the manifest references and the project
    //     does not hold fails the mount — after the reconcile, as a pod that
    //     never starts. Names only; no payload is read. Runs in `--dry-run`
    //     too, which is what makes a dry run a real activation rehearsal.
    ensure_projected_objects_resolve(&manifests)?;

    // 5. Reconcile — `kubectl diff -k` (drift review) then `apply -k`, so
    //    any manifest delta (env, sidecars, volumes, container renames)
    //    that landed in `main` reaches the cluster, and every image lands on
    //    `tag` in the same write. Always runs; no external overlay folder.
    reconcile_manifests(cfg, dry_run, &target)?;

    // 6. Wait on the rollouts the reconcile started. The apply in step 5
    //    already pinned every image to `tag` — the render substitutes it —
    //    so there is no `kubectl set image` to follow up with. One write
    //    means one ReplicaSet: no intermediate generation on an unpullable
    //    placeholder for a `maxSurge: 0` workload to be deleted for.
    wait_rollouts(cfg, dry_run, "300s", rollout_wait_deployments())?;

    // 5b. Pin every trigger CronJob to the same tag so a roll is atomic
    //     across ALL navigator images, not just the two services. CronJobs
    //     don't "roll" — the new image takes effect on the next scheduled
    //     run — so there is nothing to wait on.
    pin_cronjob_images(cfg, &tag, dry_run)?;

    // 6. Re-register the workers with Restate. The DevX notification worker
    // must register successfully: unlike the established workflows service,
    // it has no prior deployment to preserve on its first ship.
    reregister(cfg, dry_run)?;

    // 7. Smoke-check the public surface (best-effort).
    smoke_check(cfg, dry_run);

    eprintln!("==> ship complete: {tag} live in {}", cfg.project_id);
    Ok(())
}

/// Re-pin every navigator trigger `CronJob` to `tag`. Discovers the
/// `CronJobs` from the live cluster and re-points any container whose image
/// is one of ours (`<region>-docker.pkg.dev/<project>/<repo>/navigator-*`) — no hard-coded list,
/// so a newly added trigger is covered automatically. Each image is
/// re-pinned only after confirming `tag` is actually published for it;
/// a trigger whose image hasn't published `tag` yet is skipped with a
/// warning rather than wedged in `ImagePullBackOff`.
fn pin_cronjob_images(cfg: &ShipConfig, tag: &str, dry_run: bool) -> Result<()> {
    let prefix = format!("{}/", cfg.registry());
    if dry_run {
        eprintln!("DRY-RUN: would re-pin every {prefix}navigator-* CronJob image to {tag}");
        return Ok(());
    }
    let list = kubectl_list_json(cfg, "cronjobs")?;
    let Some(items) = list.get("items").and_then(serde_json::Value::as_array) else {
        eprintln!("==> no CronJobs found; nothing to pin");
        return Ok(());
    };
    let mut pinned = 0u32;
    for item in items {
        let Some(name) = item
            .pointer("/metadata/name")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let containers = item
            .pointer("/spec/jobTemplate/spec/template/spec/containers")
            .and_then(serde_json::Value::as_array);
        let Some(containers) = containers else {
            continue;
        };
        for c in containers {
            let (Some(cname), Some(image)) = (
                c.get("name").and_then(serde_json::Value::as_str),
                c.get("image").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            // Ours only: `<registry>/navigator-<something>:<tag>`.
            if !image.starts_with(&prefix) {
                continue;
            }
            let base = image.rsplit_once(':').map_or(image, |(b, _)| b);
            let short = base.strip_prefix(&prefix).unwrap_or(base);
            if registry::tag_exists(&cfg.registry, short, tag) {
                let target = format!("{base}:{tag}");
                exec(
                    dry_run,
                    kubectl(cfg)
                        .arg("set")
                        .arg("image")
                        .arg(format!("cronjob/{name}"))
                        .arg(format!("{cname}={target}")),
                )?;
                pinned += 1;
            } else {
                eprintln!(
                    "WARN: {short} has no {tag} tag in the registry — leaving CronJob/{name} on {image}"
                );
            }
        }
    }
    eprintln!("==> pinned {pinned} trigger CronJob image(s) to {tag}");
    Ok(())
}

/// 3 — reconcile the full manifest tree. Render the embedded GKE + base
/// manifests with the deployer's `NAVIGATOR_*` values into a throwaway
/// temp dir, `kubectl diff -k` it (surface drift), then — unless
/// `--dry-run` — `kubectl apply -k` it. The apply is unconditional, so
/// any structural manifest change (a renamed container, a new sidecar or
/// volume, an env-list edit) that landed in `main` reaches the cluster
/// instead of silently rotting. There is no external overlay folder and no
/// image-only fall-through: the CLI generates from env what a deployer used
/// to hand-keep in a private overlay.
///
/// The rendered temp dir is dropped on return, so the substituted manifests
/// never linger on disk. A missing required substitution var bails by name
/// before anything is written (see [`resolve_substitutions`]).
fn reconcile_manifests(cfg: &ShipConfig, dry_run: bool, target: &Path) -> Result<()> {
    eprintln!(
        "==> reconciling manifests — rendered embedded tree → {}",
        target.display()
    );
    reconcile_kustomize(dry_run, |verb| {
        let status = kubectl_ctx(cfg)
            .arg(verb)
            .arg("-k")
            .arg(target)
            .status()
            .with_context(|| format!("spawn kubectl {verb} -k {}", target.display()))?;
        Ok(status.code().unwrap_or(-1))
    })
}

/// Drive the reconcile's `kubectl -k` verbs in order. `run_verb` returns the
/// process exit code (or `Err` if kubectl could not be spawned); the real
/// ship shells out to `kubectl <verb> -k <target>`, tests pass a stub.
///
/// `kubectl diff` has three outcomes: exit 0 (no drift), 1 (drift found —
/// the normal, benign signal), and >1 (a real error: unreachable context,
/// auth failure, a kustomize build error). Exit 0/1 proceed; a >1 exit or a
/// spawn failure **aborts** the reconcile — the diff is the operator's
/// mandatory drift-review gate, so a diff that never ran must not let a
/// `--dry-run` exit 0 (mistaking a missing check for "no changes") or a live
/// run fall through to `apply` having skipped review. `apply` must likewise
/// exit 0 or the ship aborts (a partial reconcile is worse than none). Under
/// `--dry-run` the verb list stops at `diff` ([`reconcile_verbs`]), so
/// nothing is applied.
fn reconcile_kustomize<R>(dry_run: bool, mut run_verb: R) -> Result<()>
where
    R: FnMut(&str) -> Result<i32>,
{
    for verb in reconcile_verbs(dry_run) {
        match *verb {
            "apply" => {
                let code = run_verb("apply")?;
                if code != 0 {
                    bail!(
                        "kubectl apply -k failed (exit {code}); the cluster was NOT fully \
                         reconciled — inspect the output above before retrying"
                    );
                }
            }
            // `diff`: 0/1 are the expected outcomes (no-drift / drift); a
            // higher code or a spawn failure means the diff itself broke, so
            // the mandatory drift-review gate never ran — abort rather than
            // fall through to `apply` (live) or exit 0 (`--dry-run`).
            "diff" => {
                match run_verb("diff") {
                    Ok(0 | 1) => {}
                    Ok(code) => bail!(
                    "kubectl diff -k failed (exit {code}); the rendered manifests were NOT checked \
                     against the cluster — inspect the output above before retrying"
                ),
                    Err(err) => return Err(err).context(
                        "kubectl diff -k could not run; the rendered manifests were NOT checked \
                         against the cluster",
                    ),
                }
            }
            _ => {}
        }
    }
    if dry_run {
        eprintln!(
            "DRY-RUN: manifests rendered and diffed above; skipping `kubectl apply` \
             (rendered dir removed on exit)"
        );
    }
    Ok(())
}

/// 7b — abort the ship if any web-binary boot requirement is
/// unsatisfied by both the Secret and that Deployment's own env. An
/// unsatisfied requirement crash-loops the new pod at boot
/// (`enforce_deployment_invariants`), so catching it here beats a
/// silently-stalled rollout. We never auto-patch: print the exact
/// `kubectl patch` and stop.
fn ensure_secret_invariants(cfg: &ShipConfig, dry_run: bool, manifests: &str) -> Result<()> {
    if dry_run {
        eprintln!(
            "DRY-RUN: checking required keys vs Secret + each web-binary Deployment env (read-only)"
        );
    }
    let secret_keys = secret_data_keys(cfg)?;
    check_secret_invariants(cfg, manifests, &secret_keys)
}

/// One Secret Manager object a rendered `SecretProviderClass` references,
/// spelled the way the CSI driver will request it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ProjectedObject {
    project_id: String,
    secret_id: String,
    version: String,
}

impl ProjectedObject {
    /// Parse a `resourceName`. The whole coordinate is read rather than just
    /// the object name: the project is what makes the check ask about the
    /// deployment actually being shipped, and the version is what the driver
    /// pins — assuming `latest` would check something the mount never reads.
    fn parse(resource_name: &str) -> Result<Self> {
        let parts: Vec<&str> = resource_name.split('/').collect();
        match parts.as_slice() {
            ["projects", project_id, "secrets", secret_id, "versions", version]
                if !project_id.is_empty() && !secret_id.is_empty() && !version.is_empty() =>
            {
                Ok(Self {
                    project_id: (*project_id).to_owned(),
                    secret_id: (*secret_id).to_owned(),
                    version: (*version).to_owned(),
                })
            }
            _ => bail!(
                "`{resource_name}` is not a Secret Manager resource name \
                 (projects/<project>/secrets/<object>/versions/<version>). A malformed reference \
                 would be silently unresolvable, so the ship stops here rather than mounting it."
            ),
        }
    }
}

/// Every Secret Manager object the rendered manifest stream's
/// `SecretProviderClass` objects reference.
///
/// Read from the *built* stream rather than the manifest file, so an object is
/// only checked when the kustomization actually applies the class. Before the
/// CSI resource is wired in there is nothing to resolve and this is empty;
/// after it is, this is exactly the set the driver will request at mount time.
/// Pure, so the parse is unit-tested against a fixture rather than a cluster.
pub(super) fn referenced_secret_manager_objects(
    manifests: &str,
) -> Result<BTreeSet<ProjectedObject>> {
    use serde::Deserialize;
    let mut referenced = BTreeSet::new();
    for document in serde_yaml::Deserializer::from_str(manifests) {
        let value = serde_json::Value::deserialize(document)
            .context("parse a document of the rendered manifest stream")?;
        if value.get("kind").and_then(serde_json::Value::as_str) != Some("SecretProviderClass") {
            continue;
        }
        let block = value
            .pointer("/spec/parameters/secrets")
            .and_then(serde_json::Value::as_str)
            .context("a SecretProviderClass carries spec.parameters.secrets")?;
        let entries: serde_json::Value = serde_yaml::from_str(block)
            .context("parse a SecretProviderClass spec.parameters.secrets block")?;
        for entry in entries
            .as_array()
            .context("spec.parameters.secrets is a list")?
        {
            let resource_name = entry
                .get("resourceName")
                .and_then(serde_json::Value::as_str)
                .context("a spec.parameters.secrets entry carries a resourceName")?;
            referenced.insert(ProjectedObject::parse(resource_name)?);
        }
    }
    Ok(referenced)
}

/// 4c — abort the ship if any object the rendered `SecretProviderClass`
/// references does not resolve to an `ENABLED` version in that deployment's
/// project.
///
/// A CSI mount fails outright on a single missing object, and it fails at
/// mount time: the pod never starts, `web` reports nothing, and the operator
/// reads an `ImagePullBackOff`-shaped symptom that has nothing to do with the
/// image. Resolving the names here turns that into a named abort before the
/// reconcile has touched anything, which is the difference between "deploy
/// failed loudly" and "the first request failed".
///
/// The check is names-only: it reads each version's state and never accesses a
/// payload, so it needs `secretmanager.versions.get` and not
/// `secretmanager.versions.access`, and no projected credential enters this
/// process.
fn ensure_projected_objects_resolve(manifests: &str) -> Result<()> {
    let referenced = referenced_secret_manager_objects(manifests)?;
    if referenced.is_empty() {
        eprintln!(
            "==> no SecretProviderClass in the rendered tree — no Secret Manager objects to resolve"
        );
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    let states = runtime.block_on(async {
        let token = super::gcp::auth::adc_token_provider().await?;
        let client = super::gcp::client::GcpClient::new(token);
        let mut states = Vec::new();
        for object in referenced {
            let state = super::gcp::secret_manager::version_state(
                &client,
                &object.project_id,
                &object.secret_id,
                &object.version,
            )
            .await?;
            states.push((object, state));
        }
        Ok::<_, anyhow::Error>(states)
    })?;

    check_projected_objects_resolve(&states)
}

/// The abort-or-proceed decision itself, split from the Secret Manager reads
/// that feed it so the thing that stops a bad ship is unit-tested against a
/// fixture rather than only against live GCP.
///
/// Every unresolved object is reported, not just the first: the failure this
/// guards is a *count* mismatch between what the manifest references and what
/// the project holds, and an operator fixing them one abort at a time learns
/// the count the slow way.
fn check_projected_objects_resolve(states: &[(ProjectedObject, Option<String>)]) -> Result<()> {
    let unresolved: Vec<String> = states
        .iter()
        .filter_map(|(object, state)| match state.as_deref() {
            Some("ENABLED") => None,
            Some(state) => Some(format!("{} ({state})", object.secret_id)),
            None => Some(format!("{} (no such object)", object.secret_id)),
        })
        .collect();
    if unresolved.is_empty() {
        eprintln!(
            "==> Secret Manager objects OK ({} referenced, {} resolved to an ENABLED version)",
            states.len(),
            states.len()
        );
        return Ok(());
    }
    let project_ids: BTreeSet<&str> = states
        .iter()
        .map(|(object, _)| object.project_id.as_str())
        .collect();
    bail!(
        "the rendered SecretProviderClass references {} Secret Manager object(s) but only {} \
         resolve to an ENABLED version in {}: {}.\n\
         A CSI mount fails outright on any object it cannot read, so this would crash-loop the \
         pod after the reconcile rather than fail here. Write the missing values with \
         `navigator ops secrets apply --deployment <name>`, or trim the objects this deployment \
         does not carry from examples/deploy/k8s/gke/secrets/secret-provider-class.yaml. \
         Nothing was applied.",
        states.len(),
        states.len() - unresolved.len(),
        project_ids.into_iter().collect::<Vec<_>>().join(", "),
        unresolved.join(", ")
    )
}

/// The preflight decision itself, split from the two shell-outs that feed
/// it (the Secret read and `kubectl kustomize`) so the whole abort-or-
/// proceed choice — the thing that stops a bad ship — is unit-tested
/// against a manifest fixture rather than only against a live cluster.
fn check_secret_invariants(
    cfg: &ShipConfig,
    manifests: &str,
    secret_keys: &BTreeSet<String>,
) -> Result<()> {
    let parsed = shared_web_requirements(&cfg.project_id);
    let deployment_envs = parse_web_binary_envs(manifests, &cfg.secret_name, secret_keys)?;
    let missing_by_deployment =
        missing_requirements_by_deployment(&parsed, secret_keys, &deployment_envs);
    if missing_by_deployment.is_empty() {
        eprintln!(
            "==> Secret invariants OK ({} web-binary deployment(s) checked)",
            deployment_envs.len()
        );
        return Ok(());
    }
    bail!(unsatisfied_secret_error(cfg, &missing_by_deployment))
}

/// The operator-facing text for an unsatisfied boot requirement. Pure —
/// every input is already resolved — so the exact string the operator
/// reads is asserted by unit tests instead of only by a failed prod ship.
fn unsatisfied_secret_error(
    cfg: &ShipConfig,
    missing_by_deployment: &[(String, Vec<SecretRequirement>)],
) -> String {
    let described: Vec<String> = missing_by_deployment
        .iter()
        .map(|(deployment, missing)| {
            let missing = missing
                .iter()
                .map(SecretRequirement::describe)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{deployment}: {missing}")
        })
        .collect();
    format!(
        "the new binary has boot requirements satisfied by neither the `{secret}` Secret \
         nor the target Deployment's env: {described:?}\n\
         (`A or B + C` boots when either the single key A or both B and C are present.)\n\
         An unsatisfied requirement crash-loops the new pod at boot. Add the missing keys \
         (values never transit logs) before re-running ship — one patch covers every \
         deployment listed above, which share the Secret:\n  \
         kubectl --context {ctx} -n {ns} patch secret {secret} --type=merge \\\n    \
         -p '{patch}'\n\
         (Keys provided as deployment env belong in the rendered manifests the reconcile applies, \
         not the Secret.)",
        secret = cfg.secret_name,
        ctx = cfg.context,
        ns = cfg.namespace,
        patch = secret_patch_stringdata(&suggested_patch_keys(missing_by_deployment)),
    )
}

/// The keys the suggested `kubectl patch` carries: for every unsatisfied
/// requirement, the FIRST alternative's keys — the canonical way to
/// satisfy it (`A or B + C` suggests `A`; the operator swaps in `B + C`
/// if that is the shape they hold). Deduped in source order, because the
/// web-binary deployments share one Secret and so surface the same
/// requirement once each.
fn suggested_patch_keys(missing_by_deployment: &[(String, Vec<SecretRequirement>)]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for key in missing_by_deployment
        .iter()
        .flat_map(|(_, missing)| missing.iter())
        .filter_map(|req| req.any_of.first())
        .flatten()
    {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    // A requirement with no alternatives can't name a key; keep the
    // command shape valid rather than emitting an empty patch.
    if keys.is_empty() {
        keys.push("KEY".to_string());
    }
    keys
}

/// The `--type=merge` patch body for `keys` — every missing key mapped to
/// a `<value>` placeholder, so one paste-and-fill sets them all.
fn secret_patch_stringdata(keys: &[String]) -> String {
    let pairs = keys
        .iter()
        .map(|key| format!("\"{key}\":\"<value>\""))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"stringData\":{{{pairs}}}}}")
}

/// Build the rendered tree into a manifest stream — the state `apply -k`
/// is about to write.
///
/// `kubectl kustomize` (not a hand-parse of the tree) so the overlay's
/// patches — notably `patches/web-env.yaml`, which `$patch: replace`s the
/// whole env list — resolve exactly as the apply will.
///
/// `pub(super)` because the KIND overlays need the same treatment for the
/// same reason: `mod.rs`'s render tests build `k8s/overlays/kind-private`
/// to prove the private-mode component actually merges there too.
pub(super) fn kustomize_build(target: &Path) -> Result<String> {
    let out = Command::new("kubectl")
        .arg("kustomize")
        .arg(target)
        .output()
        .with_context(|| format!("run kubectl kustomize {}", target.display()))?;
    if !out.status.success() {
        bail!(
            "kubectl kustomize {} failed: {}",
            target.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).context("kubectl kustomize output is not UTF-8")
}

/// Pull one document out of a rendered manifest stream by kind + name.
///
/// Sits beside [`kustomize_build`] rather than inside a test module because
/// both the GKE tests here and the KIND overlay tests in `mod.rs` assert
/// against a built stream, and one shared reader keeps them asserting on
/// the same thing.
#[cfg(test)]
pub(super) fn manifest_doc(manifests: &str, kind: &str, name: &str) -> serde_yaml::Value {
    use serde::Deserialize;
    for document in serde_yaml::Deserializer::from_str(manifests) {
        let Ok(value) = serde_yaml::Value::deserialize(document) else {
            continue;
        };
        let matches_kind = value.get("kind").and_then(serde_yaml::Value::as_str) == Some(kind);
        let matches_name = value
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(serde_yaml::Value::as_str)
            == Some(name);
        if matches_kind && matches_name {
            return value;
        }
    }
    panic!("no {kind}/{name} in the rendered stream");
}

/// The web-binary Deployments in a rendered manifest stream, each with its
/// usable env names. Pure — split from the `kubectl kustomize` shell-out so
/// the parse is unit-tested against a fixture instead of a live cluster.
///
/// The env is read from the rendered manifests rather than the live cluster
/// because the manifests are the state the reconcile is about to apply.
/// That is what lets the preflight run BEFORE any mutation: the live
/// Deployment's env is both stale (the apply is about to overwrite it) and
/// absent entirely on a first-ever ship, whereas the rendered tree is
/// authoritative in both cases.
fn parse_web_binary_envs(
    manifests: &str,
    secret_name: &str,
    secret_keys: &BTreeSet<String>,
) -> Result<Vec<(String, BTreeSet<String>)>> {
    use serde::Deserialize;
    let mut envs = Vec::new();
    for document in serde_yaml::Deserializer::from_str(manifests) {
        let value = serde_json::Value::deserialize(document)
            .context("parse a document of the rendered manifest stream")?;
        if value.get("kind").and_then(serde_json::Value::as_str) != Some("Deployment") {
            continue;
        }
        let Some(name) = value
            .pointer("/metadata/name")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        // `workflows-service` runs the worker binary, not `web`, so it is
        // not subject to the web boot invariants.
        if !web_binary_deployments().contains(&name) {
            continue;
        }
        envs.push((
            name.to_string(),
            populated_env_names(&value, secret_name, secret_keys),
        ));
    }
    if envs.is_empty() {
        bail!(
            "the rendered manifests define none of {:?} — refusing to ship a tree whose web tier \
             cannot be boot-checked",
            web_binary_deployments()
        );
    }
    Ok(envs)
}

/// The data keys carrying a non-empty value in the prod Secret.
fn secret_data_keys(cfg: &ShipConfig) -> Result<BTreeSet<String>> {
    let json = kubectl_json(cfg, "secret", &cfg.secret_name)?;
    Ok(populated_secret_keys(&json))
}

/// The Secret's `data` keys whose value is non-empty. An empty value is
/// as fatal as a missing key — `enforce_deployment_invariants` treats an empty
/// env var as unset — so counting the bare key name here would pass the
/// preflight and still crash-loop the pod at boot. (No base64 decode
/// needed: the empty value encodes to the empty string.)
fn populated_secret_keys(secret: &serde_json::Value) -> BTreeSet<String> {
    secret
        .get("data")
        .and_then(serde_json::Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, value)| value.as_str().is_some_and(|v| !v.is_empty()))
                .map(|(key, _)| key.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The container env names that can satisfy a boot requirement (see
/// [`env_var_is_usable`] for the per-declaration rule). A name-only or
/// empty-literal declaration is excluded — the pod sees an empty string,
/// which `enforce_deployment_invariants` treats as unset.
fn populated_env_names(
    deployment: &serde_json::Value,
    secret_name: &str,
    secret_keys: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Some(containers) = deployment
        .pointer("/spec/template/spec/containers")
        .and_then(serde_json::Value::as_array)
    {
        for container in containers {
            if let Some(env) = container.get("env").and_then(serde_json::Value::as_array) {
                for var in env {
                    if !env_var_is_usable(var, secret_name, secret_keys) {
                        continue;
                    }
                    if let Some(name) = var.get("name").and_then(serde_json::Value::as_str) {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names
}

/// Whether a single container env declaration resolves to a value the
/// boot invariant would accept:
///
/// - a non-empty literal `value` — usable;
/// - a `secretKeyRef` into the very Secret `ship` inspects — resolvable,
///   so usable only when the referenced key carries a non-empty value
///   (an empty one is as fatal as a missing key at boot, and counting it
///   here would let the preflight pass while the pod crash-loops);
/// - any other `valueFrom` (a different Secret, a `ConfigMap`, the
///   downward API) — the preflight cannot inspect the resolved value, so stay
///   optimistic and count it;
/// - a name-only or empty-literal declaration — not usable.
fn env_var_is_usable(
    var: &serde_json::Value,
    secret_name: &str,
    secret_keys: &BTreeSet<String>,
) -> bool {
    if let Some(value_from) = var.get("valueFrom") {
        if let Some(secret_ref) = value_from.get("secretKeyRef") {
            if secret_ref.get("name").and_then(serde_json::Value::as_str) == Some(secret_name) {
                return secret_ref
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|key| secret_keys.contains(key));
            }
        }
        return true;
    }
    var.get("value")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|v| !v.is_empty())
}

struct RestateDeployment {
    name: &'static str,
    url: String,
}

fn reregister_targets(cfg: &ShipConfig) -> [RestateDeployment; 1] {
    // One worker, every service. The folded-in GitHub webhook notice services
    // (`DevxIssueTriage`, `devx-pr`) register with `workflows-service`, so there
    // is no separate worker endpoint to register.
    [RestateDeployment {
        name: WORKFLOWS_DEPLOYMENT,
        url: cfg.workflows_url_resolved(),
    }]
}

fn apply_registration_result(deployment: &RestateDeployment, result: Result<()>) -> Result<()> {
    result.with_context(|| {
        format!(
            "Restate re-register of {} failed; its handler list would be stale — missing any \
             service added since the last registration, e.g. the folded-in GitHub webhook notice \
             services (`DevxIssueTriage`, `devx-pr`) — so webhook submissions would silently fail. \
             Refusing to complete the ship.",
            deployment.name
        )
    })
}

/// 7d — register the workflows worker with Restate so handlers added since the
/// last registration are reachable, including the folded-in GitHub webhook
/// notice services. REQUIRED: a failed re-register leaves Restate on a stale
/// handler list that would silently drop webhook submissions (the Heartbeat
/// canary tests only its own handler, not the full service list), so the ship
/// fails rather than complete with the new services unreachable.
fn reregister(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    let deployments = reregister_targets(cfg);
    if dry_run {
        for deployment in deployments {
            eprintln!(
                "DRY-RUN: would re-register {} with Restate (devx restate register {})",
                deployment.name, deployment.url
            );
        }
        return Ok(());
    }
    // The admin REST API path (RESTATE_ADMIN_URL + RESTATE_ADMIN_TOKEN) needs
    // no `restate` CLI, so only require the CLI when those env vars are absent.
    // These are operator-session credentials like ADC or a `restate cloud
    // login` token — never deployment coordinates, which all come from the
    // `deployments/` tree. Ship reads no secret value from the repository.
    let has_admin_api = !std::env::var("RESTATE_ADMIN_URL")
        .unwrap_or_default()
        .trim()
        .is_empty()
        && !std::env::var("RESTATE_ADMIN_TOKEN")
            .unwrap_or_default()
            .trim()
            .is_empty();
    if !has_admin_api && !tool_present("restate") {
        bail!(
            "no RESTATE_ADMIN_URL/TOKEN and `restate` CLI not on PATH; cannot register {WORKFLOWS_DEPLOYMENT}"
        );
    }
    // Pass every resolved URL explicitly so ship targets the same endpoints
    // it printed, independent of the workflows-only environment resolver.
    for deployment in &deployments {
        apply_registration_result(deployment, super::restate_register(Some(&deployment.url)))?;
    }
    Ok(())
}

/// 8 — curl the public landing and grep a fixed phrase. Best-effort:
/// reports, never fails the ship.
fn smoke_check(cfg: &ShipConfig, dry_run: bool) {
    let url = format!("https://{}/", cfg.public_host);
    if dry_run {
        eprintln!("DRY-RUN: would smoke-check {url}");
        return;
    }
    if !tool_present("curl") {
        eprintln!("WARN: `curl` not on PATH; skipping smoke check of {url}");
        return;
    }
    // Confirm the public landing is non-empty by grepping a stable phrase.
    // `--max-time` bounds the whole request so a stalled or unreachable
    // host can't hang the ship after the rollout already succeeded.
    let phrase = "home";
    match Command::new("curl")
        .args(["-fsS", "--max-time", "20", &url])
        .output()
    {
        Ok(out) if out.status.success() => {
            let body = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
            if body.contains(phrase) {
                eprintln!("==> smoke check OK ({url})");
            } else {
                eprintln!("WARN: {url} returned 200 but the expected phrase was absent — inspect the page");
            }
        }
        Ok(out) => eprintln!("WARN: smoke check non-2xx for {url}: {}", out.status),
        Err(err) => eprintln!("WARN: smoke check could not reach {url}: {err}"),
    }
    eprintln!("==> workflows-service has no public /; confirm it is ready:");
    eprintln!(
        "    kubectl --context {} -n {} get pods -l app={WORKFLOWS_DEPLOYMENT}",
        cfg.context, cfg.namespace
    );
}

// ---------- small shared helpers ----------

/// A `kubectl` invocation pinned to the prod context and namespace.
fn kubectl(cfg: &ShipConfig) -> Command {
    let mut cmd = kubectl_ctx(cfg);
    cmd.arg("-n").arg(&cfg.namespace);
    cmd
}

/// A `kubectl` invocation pinned to the prod context only (for the
/// kustomize `diff`/`apply`, which carry their own namespaces).
fn kubectl_ctx(cfg: &ShipConfig) -> Command {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--context").arg(&cfg.context);
    cmd
}

/// `kubectl get <kind> <name> -o json`, parsed.
fn kubectl_json(cfg: &ShipConfig, kind: &str, name: &str) -> Result<serde_json::Value> {
    let out = kubectl(cfg)
        .arg("get")
        .arg(kind)
        .arg(name)
        .arg("-o")
        .arg("json")
        .output()
        .with_context(|| format!("run kubectl get {kind} {name}"))?;
    if !out.status.success() {
        bail!(
            "kubectl get {kind} {name} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parse `kubectl get {kind} {name} -o json`"))
}

/// `kubectl get <kind> -o json` for a whole collection (no name), parsed.
/// The result is a `List` whose `items` array the caller walks.
fn kubectl_list_json(cfg: &ShipConfig, kind: &str) -> Result<serde_json::Value> {
    let out = kubectl(cfg)
        .arg("get")
        .arg(kind)
        .arg("-o")
        .arg("json")
        .output()
        .with_context(|| format!("run kubectl get {kind}"))?;
    if !out.status.success() {
        bail!(
            "kubectl get {kind} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parse `kubectl get {kind} -o json`"))
}

/// Wait on the selected Deployments' rollouts at the given timeout. On a
/// rollout that fails or times out, dump pod/event/log diagnostics for the
/// namespace before surfacing the error — so a wedged roll shows WHY
/// (crash-loop, `ImagePullBackOff`, Pending on no capacity) on the spot,
/// instead of a bare "timed out waiting for the condition". This is the
/// operator-side mirror of `deploy.yml`'s "dump diagnostics on failure".
fn wait_rollouts(
    cfg: &ShipConfig,
    dry_run: bool,
    timeout: &str,
    deployments: &[&str],
) -> Result<()> {
    for deployment in deployments {
        let result = exec(
            dry_run,
            kubectl(cfg)
                .arg("rollout")
                .arg("status")
                .arg(format!("deployment/{deployment}"))
                .arg(format!("--timeout={timeout}")),
        );
        if let Err(err) = result {
            if !dry_run {
                dump_rollout_diagnostics(cfg, deployment);
            }
            return Err(err).with_context(|| {
                format!("deployment/{deployment} did not become Ready within {timeout}")
            });
        }
    }
    Ok(())
}

/// Print pod status, recent events, and recent container logs for the
/// namespace when a rollout wedges — the on-the-spot answer to "why is the
/// deploy hanging?". Best-effort: each call ignores its own failure so a
/// diagnostics gap never masks the real rollout error. Output streams
/// straight to the operator's terminal (inherited stdio).
fn dump_rollout_diagnostics(cfg: &ShipConfig, deployment: &str) {
    eprintln!(
        "\n==> deployment/{deployment} stalled — dumping diagnostics (namespace {})",
        cfg.namespace
    );
    eprintln!("--- kubectl get pods ---");
    let _ = kubectl(cfg)
        .arg("get")
        .arg("pods")
        .arg("-o")
        .arg("wide")
        .status();
    // kubectl has no `--tail` for events, so capture the time-sorted
    // stream and tail it in Rust — no `sh -c`, so cfg.context/namespace
    // are passed as argv (never interpolated into a shell string) and
    // stay safe even when a fork's NAVIGATOR_GKE_CONTEXT carries
    // whitespace or shell metacharacters.
    eprintln!("--- recent events (last 25) ---");
    if let Ok(out) = kubectl(cfg)
        .arg("get")
        .arg("events")
        .arg("--sort-by=.lastTimestamp")
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(25)..] {
            eprintln!("{line}");
        }
    }
    eprintln!("--- recent logs: deployment/{deployment} (last 80 lines) ---");
    let _ = kubectl(cfg)
        .arg("logs")
        .arg(format!("deployment/{deployment}"))
        .arg("--all-containers")
        .arg("--tail=80")
        .status();
    // A crash-looped pod's fatal output is in the PREVIOUS container, not
    // the one currently restarting — pull it too (no-op if not crashed).
    eprintln!("--- previous (crashed) container logs, if any ---");
    let _ = kubectl(cfg)
        .arg("logs")
        .arg(format!("deployment/{deployment}"))
        .arg("--all-containers")
        .arg("--previous")
        .arg("--tail=40")
        .status();
    eprintln!(
        "==> dig deeper: kubectl --context {} -n {} describe pods -l app={deployment}\n",
        cfg.context, cfg.namespace
    );
}

/// Confirm the resolved `kubectl` context exists before any prod call.
/// A deterministic ship must not silently land on whatever context
/// happens to be current.
/// Whether the pinned context names the coordinates this ship resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ContextCheck {
    /// A `gke_<project>_<location>_<cluster>` context naming exactly the
    /// resolved project, location, and cluster.
    Matches,
    /// A `gke_…` context naming a *different* cluster. The deployment
    /// coordinates and the cluster the images would land on disagree.
    Mismatch {
        context_project: String,
        context_location: String,
        context_cluster: String,
    },
    /// Not a `get-credentials` context name, so its text carries no
    /// coordinates to cross-check.
    Unverifiable,
}

/// `gcloud container clusters get-credentials <cluster> --region <location>
/// --project <project>` names the context `gke_<project>_<location>_<cluster>`.
/// Project IDs, regions, and cluster names may all contain `-` but never `_`,
/// so the four-way split is unambiguous.
fn check_context(context: &str, project: &str, location: &str, cluster: &str) -> ContextCheck {
    let parts: Vec<&str> = context.split('_').collect();
    let [prefix, context_project, context_location, context_cluster] = parts.as_slice() else {
        return ContextCheck::Unverifiable;
    };
    if *prefix != "gke" {
        return ContextCheck::Unverifiable;
    }
    if *context_project == project && *context_location == location && *context_cluster == cluster {
        return ContextCheck::Matches;
    }
    ContextCheck::Mismatch {
        context_project: (*context_project).to_owned(),
        context_location: (*context_location).to_owned(),
        context_cluster: (*context_cluster).to_owned(),
    }
}

/// The resolved coordinates, printed before anything acts on them. Every
/// deployment in the `deployments/` tree runs in its own project; an
/// operator has to be able to see which one `--deployment` selected.
fn resolved_coordinates(cfg: &ShipConfig) -> String {
    format!(
        "==> resolved deployment\n      name:      {}\n      project:   {}\n      location:  {}\n      \
         cluster:   {}\n      namespace: {}\n      context:   {}\n      images:    {}",
        cfg.name, cfg.project_id, cfg.location, cfg.cluster, cfg.namespace, cfg.context,
        cfg.registry
    )
}

fn context_mismatch_error(cfg: &ShipConfig, check: &ContextCheck) -> String {
    let ContextCheck::Mismatch {
        context_project,
        context_location,
        context_cluster,
    } = check
    else {
        unreachable!("only a mismatch produces an error");
    };
    format!(
        "kubectl context '{}' names cluster {context_cluster} in {context_project}/{context_location}, \
         but this deployment resolved cluster {} in {}/{}. Shipping would roll {}'s release onto another \
         deployment's cluster. Fix NAVIGATOR_GKE_CONTEXT in deployments/{name}/config.toml, or get \
         credentials for the right cluster:\n    gcloud container clusters get-credentials {} --region {} --project {}",
        cfg.context,
        cfg.cluster,
        cfg.project_id,
        cfg.location,
        cfg.project_id,
        cfg.cluster,
        cfg.location,
        cfg.project_id,
        name = cfg.name,
    )
}

fn verify_context(cfg: &ShipConfig, dry_run: bool) -> Result<()> {
    eprintln!("{}", resolved_coordinates(cfg));
    // The coordinate cross-check is pure text, so it runs in dry-run too —
    // that is the mode an operator uses to confirm the target before a real
    // ship, and it is where a copied context should surface.
    let check = check_context(&cfg.context, &cfg.project_id, &cfg.location, &cfg.cluster);
    match &check {
        ContextCheck::Matches => {}
        ContextCheck::Mismatch { .. } => bail!("{}", context_mismatch_error(cfg, &check)),
        ContextCheck::Unverifiable => eprintln!(
            "==> NOTE: context '{}' is not a `gke_<project>_<location>_<cluster>` name, so its \
             coordinates could not be cross-checked against the resolved deployment",
            cfg.context
        ),
    }
    if dry_run {
        eprintln!("DRY-RUN: would pin kubectl context → {}", cfg.context);
        return Ok(());
    }
    let out = Command::new("kubectl")
        .args(["config", "get-contexts", "-o", "name"])
        .output()
        .context("kubectl config get-contexts")?;
    if !out.status.success() {
        bail!(
            "kubectl config get-contexts failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let contexts = String::from_utf8_lossy(&out.stdout);
    if !contexts.lines().any(|c| c == cfg.context) {
        bail!(
            "kubectl context '{}' not found. Get prod credentials \
             (`gcloud container clusters get-credentials …`) or set NAVIGATOR_GKE_CONTEXT \
             to the right context name.",
            cfg.context
        );
    }
    eprintln!("==> pinning kubectl context → {}", cfg.context);
    Ok(())
}

/// True when `tool` is on PATH (same probe as `require_tools`, but
/// boolean — for best-effort steps that downgrade to a warning).
fn tool_present(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The `navigator-web` Google service account — the Workload Identity
/// principal the web pod runs as (KSA-bound in the Deployment; see
/// `web_pod_binds_the_navigator_web_service_account`).
fn web_gsa_email(project_id: &str, account_id: &str) -> String {
    format!("{account_id}@{project_id}.iam.gserviceaccount.com")
}

/// The role that carries `iam.serviceAccounts.signBlob` on a service account.
const SELF_SIGNING_ROLE: &str = "roles/iam.serviceAccountTokenCreator";

/// The `gcloud` argv that reads the web GSA's own IAM policy. This is the
/// steady-state call: it needs only `iam.serviceAccounts.getIamPolicy`, where
/// the binding write below also needs `setIamPolicy`. Factored out pure so it
/// is unit-testable without shelling out.
fn web_signing_iam_read_args(project_id: &str, account_id: &str) -> Vec<String> {
    vec![
        "iam".into(),
        "service-accounts".into(),
        "get-iam-policy".into(),
        web_gsa_email(project_id, account_id),
        format!("--project={project_id}"),
        "--format=json".into(),
    ]
}

/// The `gcloud` argv that lets the web GSA sign GCS URLs for itself.
/// Factored out pure so it is unit-testable without shelling out.
fn web_signing_iam_binding_args(project_id: &str, account_id: &str) -> Vec<String> {
    let gsa = web_gsa_email(project_id, account_id);
    vec![
        "iam".into(),
        "service-accounts".into(),
        "add-iam-policy-binding".into(),
        gsa.clone(),
        format!("--project={project_id}"),
        "--role".into(),
        SELF_SIGNING_ROLE.into(),
        format!("--member=serviceAccount:{gsa}"),
    ]
}

/// True when `policy` — a parsed `get-iam-policy` document — already grants
/// `gsa` the token-creator role on itself.
///
/// A binding carrying a `condition` does not count. The pod signs on every
/// document download, at an hour no expression here can predict, so a grant
/// that only sometimes applies is not the invariant step 1c asserts; treating
/// it as absent makes the roll write the unconditional binding instead of
/// passing a check the runtime may fail.
fn policy_grants_self_signing(policy: &serde_json::Value, gsa: &str) -> bool {
    let member = format!("serviceAccount:{gsa}");
    policy["bindings"].as_array().is_some_and(|bindings| {
        bindings.iter().any(|binding| {
            binding["role"].as_str() == Some(SELF_SIGNING_ROLE)
                && binding.get("condition").is_none()
                && binding["members"]
                    .as_array()
                    .is_some_and(|members| members.iter().any(|m| m.as_str() == Some(&member)))
        })
    })
}

/// What this roll is permitted to do about an absent self-binding.
///
/// The verify half is not selectable — the binding is asserted in every mode,
/// because a pod that cannot sign 500s every document download. What varies is
/// whether the roll may *establish* it, which is a different Google permission
/// (`setIamPolicy`) from the one the verify needs (`getIamPolicy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SigningIamAuthority {
    /// Write the binding when it is absent. The default, and what `ops gcp
    /// setup` has usually already made a no-op.
    #[default]
    Write,
    /// `--assert-signing-iam`: verify only. An absent binding fails the roll
    /// with the command that would establish it, rather than attempting a
    /// write the operator is not permitted.
    AssertOnly,
}

/// Read the authority out of the parsed flags. One place, so the flag's
/// meaning cannot drift between lanes.
fn signing_iam_authority(opts: &ShipOpts) -> SigningIamAuthority {
    if opts.assert_signing_iam {
        SigningIamAuthority::AssertOnly
    } else {
        SigningIamAuthority::Write
    }
}

/// The preflight error for a policy read that did not return an answer. The
/// roll stops rather than guessing: an unverified binding is exactly the state
/// that 500s every download. Names the permission, a role carrying it, and the
/// resource, because the operator's next move is to request that grant.
fn signing_iam_read_failed(gsa: &str, detail: &str) -> anyhow::Error {
    anyhow!(
        "cannot verify the GCS signing binding on {gsa}: reading its IAM policy failed. \
         `ops ship` needs `iam.serviceAccounts.getIamPolicy` on that service account \
         (carried by roles/iam.serviceAccountAdmin; roles/container.developer carries no \
         iam.serviceAccounts.* permission at all). gcloud said: {detail}"
    )
}

/// The preflight error for a binding that is genuinely missing and could not
/// be written. Distinct from the read failure above: here we know the pod
/// cannot sign, so there is nothing to roll onto until the grant lands.
/// `args` is the write argv this roll just attempted, quoted verbatim so the
/// operator can hand the exact command to someone who holds the permission.
fn signing_iam_write_failed(gsa: &str, args: &[String], detail: &str) -> anyhow::Error {
    anyhow!(
        "{gsa} is missing {SELF_SIGNING_ROLE} on itself and the binding could not be written. \
         Every document download would 500 on iam.serviceAccounts.signBlob, so the roll stops \
         here. Writing it needs `iam.serviceAccounts.setIamPolicy` on that service account \
         (carried by roles/iam.serviceAccountAdmin), or have someone holding it run: \
         `gcloud {}`. gcloud said: {detail}",
        args.join(" "),
    )
}

/// The preflight refusal for `--assert-signing-iam`. Distinct from
/// [`signing_iam_write_failed`], which reports a write this roll attempted and
/// lost: here no write was attempted at all, because the flag withdrew the
/// roll's authority to try one. The operator asked to be told rather than
/// granted for, so the message is the grant they now have to arrange —
/// the permission, a role carrying it, and the exact command, quoted verbatim
/// for whoever does hold it.
fn signing_iam_assert_failed(gsa: &str, args: &[String]) -> anyhow::Error {
    anyhow!(
        "{gsa} is missing {SELF_SIGNING_ROLE} on itself, and --assert-signing-iam withdrew \
         this roll's authority to write it. Every document download would 500 on \
         iam.serviceAccounts.signBlob, so the roll stops here rather than rolling onto a row \
         that cannot sign. Establishing the binding needs `iam.serviceAccounts.setIamPolicy` \
         on that service account (carried by roles/iam.serviceAccountAdmin) — have someone \
         holding it run: `gcloud {}`, then re-run this roll. Dropping the flag lets the roll \
         attempt that write itself.",
        args.join(" "),
    )
}

/// Ensure the web GSA can mint V4 GCS signed URLs. Under Workload
/// Identity the pod holds no private key, so the storage SDK signs each
/// document-download URL by calling IAM Credentials `signBlob` on its
/// own service account — which requires `roles/iam.serviceAccountTokenCreator`
/// on itself. Without it every `/…/documents/:doc_id/download` 500s with
/// `iam.serviceAccounts.signBlob denied`.
///
/// Verify-then-assert, not assert. `ops gcp setup` already writes this exact
/// binding, so on a provisioned row the steady state is "already bound" — and
/// gcloud's `add-iam-policy-binding` sends `setIamPolicy` unconditionally even
/// then, which would make every roll require a write permission it otherwise
/// has no use for. Reading first drops the common case to `getIamPolicy`.
///
/// The assertion is never skippable, only cheaper to confirm — but *who*
/// establishes the binding is a choice, because reading the policy and writing
/// it are different permissions. By default an absent binding is written here.
/// Under `--assert-signing-iam` it is reported and the roll stops, which is
/// the whole lane for an operator holding the release tag but no IAM write:
/// they can prove the invariant without being able to grant it.
///
/// `--dry-run` performs the read and declines only the write. The read is the
/// half a dry-run can answer honestly, so it does.
fn ensure_web_signing_iam(
    cfg: &ShipConfig,
    dry_run: bool,
    authority: SigningIamAuthority,
) -> Result<()> {
    ensure_web_signing_iam_with(cfg, dry_run, authority, || read_web_signing_policy(cfg))
}

/// Read the web GSA's IAM policy. Takes no `dry_run`, deliberately: this read
/// happens in every mode. A dry-run exists to answer "would the real roll
/// work", and skipping the one check it can perform for free is how a dry-run
/// reached `==> ship complete` for a roll that then died at step 1c. It is
/// also the permission probe — an operator missing `getIamPolicy` fails here,
/// under `--dry-run`, exactly as the live roll would.
fn read_web_signing_policy(cfg: &ShipConfig) -> Result<serde_json::Value> {
    let gsa = web_gsa_email(&cfg.project_id, &cfg.google_service_account_id);
    let out = Command::new("gcloud")
        .args(web_signing_iam_read_args(
            &cfg.project_id,
            &cfg.google_service_account_id,
        ))
        .output()
        .with_context(|| format!("run gcloud get-iam-policy {gsa}"))?;
    if !out.status.success() {
        return Err(signing_iam_read_failed(
            &gsa,
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("parse the IAM policy of {gsa}"))
}

/// The step past its read, with the read injected so a unit test can drive
/// every verdict — including the one that matters here, that `--dry-run` still
/// calls `read_policy` and still fails when that read is denied.
fn ensure_web_signing_iam_with(
    cfg: &ShipConfig,
    dry_run: bool,
    authority: SigningIamAuthority,
    read_policy: impl FnOnce() -> Result<serde_json::Value>,
) -> Result<()> {
    let gsa = web_gsa_email(&cfg.project_id, &cfg.google_service_account_id);
    eprintln!("==> verifying {gsa} can sign GCS URLs ({SELF_SIGNING_ROLE} on itself)");

    let policy = read_policy()?;
    if policy_grants_self_signing(&policy, &gsa) {
        eprintln!("==> already bound; no IAM write");
        return Ok(());
    }

    let write_args = web_signing_iam_binding_args(&cfg.project_id, &cfg.google_service_account_id);

    // Checked ahead of the dry-run branch, so the flag refuses under both
    // modes. A dry-run answers "would the real roll work", and under this flag
    // the real roll refuses — returning Ok here would be the same false green
    // that moving the read into every mode was meant to end.
    if authority == SigningIamAuthority::AssertOnly {
        return Err(signing_iam_assert_failed(&gsa, &write_args));
    }

    // The write is the only half a dry-run declines to perform, and it says so
    // loudly: an absent binding means the live roll needs `setIamPolicy`, which
    // nothing short of attempting the write can confirm the operator holds.
    if dry_run {
        eprintln!("DRY-RUN $ gcloud {}", write_args.join(" "));
        eprintln!(
            "DRY-RUN: the binding is ABSENT, so the real roll performs that write — it needs \
             iam.serviceAccounts.setIamPolicy on {gsa}, which a dry-run cannot confirm."
        );
        return Ok(());
    }

    eprintln!("==> binding absent — granting {SELF_SIGNING_ROLE}");
    let out = Command::new("gcloud")
        .args(&write_args)
        .output()
        .with_context(|| format!("run gcloud add-iam-policy-binding {gsa}"))?;
    if !out.status.success() {
        return Err(signing_iam_write_failed(
            &gsa,
            &write_args,
            String::from_utf8_lossy(&out.stderr).trim(),
        ));
    }
    Ok(())
}

/// Run a command, or — under `--dry-run` — print it instead.
fn exec(dry_run: bool, cmd: &mut Command) -> Result<()> {
    if dry_run {
        eprintln!("DRY-RUN $ {}", render_cmd(cmd));
        Ok(())
    } else {
        run(cmd)
    }
}

/// Render a `Command` as a copy-pasteable shell line for `--dry-run`.
fn render_cmd(cmd: &Command) -> String {
    let mut out = cmd.get_program().to_string_lossy().into_owned();
    for arg in cmd.get_args() {
        out.push(' ');
        out.push_str(&arg.to_string_lossy());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> ShipConfig {
        ShipConfig {
            name: "example-prod".into(),
            environment: store::DeploymentEnvironment::Production,
            project_id: "my-org-prod".into(),
            location: "us-west4".into(),
            cluster: "navigator".into(),
            registry: "ghcr.io/neon-law-source-code".into(),
            namespace: "navigator".into(),
            web_image_name: "neon-server".into(),
            public_host: "www.example.com".into(),
            google_service_account_id: "navigator-web".into(),
            primary_domain: "example.com".into(),
            secret_name: "navigator-web-secrets".into(),
            workflows_url: None,
            context: "gke_my-org-prod_us-west4_navigator".into(),
        }
    }

    #[test]
    fn a_context_naming_the_resolved_cluster_matches() {
        let cfg = sample_config();
        assert_eq!(
            check_context(&cfg.context, &cfg.project_id, &cfg.location, &cfg.cluster),
            ContextCheck::Matches
        );
    }

    #[test]
    fn a_context_from_another_deployment_refuses_the_ship() {
        // The failure this guard exists for. Deployment configs get seeded
        // by cloning each other, so a copied `NAVIGATOR_GKE_CONTEXT` is a
        // one-line edit away from rolling one deployment's release onto
        // another's cluster — across a real-matter boundary, silently,
        // because both are valid clusters and both contexts exist.
        let cfg = ShipConfig {
            project_id: "neon-law".into(),
            cluster: "neon-law-stg".into(),
            context: "gke_another-deployment_us-west4_neon-production".into(),
            ..sample_config()
        };
        let check = check_context(&cfg.context, &cfg.project_id, &cfg.location, &cfg.cluster);
        assert_eq!(
            check,
            ContextCheck::Mismatch {
                context_project: "another-deployment".into(),
                context_location: "us-west4".into(),
                context_cluster: "neon-production".into(),
            }
        );

        let message = context_mismatch_error(&cfg, &check);
        // Names BOTH sides, so the operator can see which one is wrong…
        assert!(message.contains("names cluster neon-production in another-deployment/us-west4"));
        assert!(message.contains("resolved cluster neon-law-stg in neon-law/us-west4"));
        // …and carries the exact command that fixes it.
        assert!(message.contains(
            "gcloud container clusters get-credentials neon-law-stg --region us-west4 \
             --project neon-law"
        ));
    }

    #[test]
    fn a_matching_cluster_name_in_the_wrong_project_still_refuses() {
        // Two deployments can share one GCP project and differ only by resource
        // prefix, so cluster name alone is not identity.
        let cfg = ShipConfig {
            project_id: "neon-law-stg".into(),
            cluster: "example-a".into(),
            context: "gke_neon-law_us-west4_example-a".into(),
            ..sample_config()
        };
        assert!(matches!(
            check_context(&cfg.context, &cfg.project_id, &cfg.location, &cfg.cluster),
            ContextCheck::Mismatch { .. }
        ));
    }

    #[test]
    fn a_context_that_is_not_a_get_credentials_name_cannot_be_cross_checked() {
        // A fork may rename its context; the text then carries no coordinates.
        // Unverifiable is not the same as matching — `verify_context` says so
        // out loud rather than passing quietly.
        for context in [
            "prod",
            "gke_only_three",
            "arn:aws:eks:us-west-2:1:cluster/x",
        ] {
            assert_eq!(
                check_context(context, "my-org-prod", "us-west4", "navigator"),
                ContextCheck::Unverifiable,
                "`{context}` carries no GKE coordinates"
            );
        }
    }

    #[test]
    fn the_resolved_coordinates_are_printed_before_anything_acts_on_them() {
        let printed = resolved_coordinates(&sample_config());
        for expected in [
            "project:   my-org-prod",
            "location:  us-west4",
            "cluster:   navigator",
            "namespace: navigator",
            "context:   gke_my-org-prod_us-west4_navigator",
            "images:    ghcr.io/neon-law-source-code",
        ] {
            assert!(printed.contains(expected), "missing `{expected}`");
        }
    }

    #[test]
    fn a_copied_context_aborts_the_dry_run_too() {
        // Dry-run is the mode an operator uses to confirm the target before a
        // real ship, so the cross-check must fire there — it is pure text and
        // needs no cluster.
        let cfg = ShipConfig {
            project_id: "neon-law".into(),
            cluster: "acme-stg".into(),
            context: "gke_neon-law-stg_us-west4_neon-law-stg".into(),
            ..sample_config()
        };
        let err = verify_context(&cfg, true).expect_err("a copied context must abort the dry run");
        assert!(err.to_string().contains("Shipping would roll"));
    }

    /// A full, valid substitution environment for the render tests — one
    /// value per required var, keyed by the var name `resolve_substitutions`
    /// reads.
    const FULL_ENV: &[(&str, &str)] = &[
        ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-420305"),
        ("NAVIGATOR_IMAGES_PROJECT_ID", "ghcr"),
        ("NAVIGATOR_GAR_REPO", "navigator"),
        ("NAVIGATOR_WEB_IMAGE", "neon-server"),
        ("NAVIGATOR_PUBLIC_HOST", "www.neonlaw.com"),
        ("NAVIGATOR_WORKFLOWS_HOST", "workflows.neonlaw.com"),
        ("NAVIGATOR_DOCUMENTS_BUCKET", "neon-production-documents"),
        (
            "NAVIGATOR_APPLICATIONS_BUCKET",
            "neon-production-applications",
        ),
        ("NAVIGATOR_ASSETS_BUCKET", "neon-production-assets"),
        ("NAVIGATOR_ASSET_BASE_URL", "https://www.neonlaw.com/assets"),
        ("NAVIGATOR_EXPORTS_BUCKET", "neon-production-exports"),
        ("NAVIGATOR_GATEWAY_IP_NAME", "neon-production-gateway-ip"),
        ("NAVIGATOR_GCP_SERVICE_ACCOUNT_ID", "neon-production-web"),
        ("NAVIGATOR_WEB_SECRET_NAME", "neon-production-web-secrets"),
        ("GOOGLE_OAUTH_REQUIRED_HD", "neonlaw.com"),
        ("NAVIGATOR_K8S_NAMESPACE", "neon-production"),
        ("NAVIGATOR_GCP_LOCATION", "us-west4"),
        (
            "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
            "111-browser.apps.googleusercontent.com",
        ),
        (
            "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI",
            "222-gemini.apps.googleusercontent.com",
        ),
    ];

    /// The two-project shape every real environment now runs: images in the
    /// hub, everything else in the environment's own project. `FULL_ENV`
    /// above deliberately omits the override so the single-project fallback
    /// stays covered by every test that uses it.
    const HUB_ENV: &[(&str, &str)] = &[
        ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg"),
        ("NAVIGATOR_IMAGES_PROJECT_ID", "ghcr"),
        ("NAVIGATOR_GAR_REPO", "navigator"),
        ("NAVIGATOR_WEB_IMAGE", "neon-server"),
        ("NAVIGATOR_PUBLIC_HOST", "staging.neonlaw.com"),
        ("NAVIGATOR_WORKFLOWS_HOST", "workflows-staging.neonlaw.com"),
        ("NAVIGATOR_DOCUMENTS_BUCKET", "neon-law-stg-documents"),
        ("NAVIGATOR_APPLICATIONS_BUCKET", "neon-law-stg-applications"),
        ("NAVIGATOR_ASSETS_BUCKET", "neon-law-stg-assets"),
        (
            "NAVIGATOR_ASSET_BASE_URL",
            "https://staging.neonlaw.com/assets",
        ),
        ("NAVIGATOR_EXPORTS_BUCKET", "neon-law-stg-exports"),
        ("NAVIGATOR_GATEWAY_IP_NAME", "neon-law-stg-gateway-ip"),
        ("NAVIGATOR_GCP_SERVICE_ACCOUNT_ID", "neon-law-stg-web"),
        ("NAVIGATOR_WEB_SECRET_NAME", "neon-law-stg-web-secrets"),
        ("GOOGLE_OAUTH_REQUIRED_HD", "neonlaw.com"),
        ("NAVIGATOR_K8S_NAMESPACE", "neon-law-stg"),
        ("NAVIGATOR_GCP_LOCATION", "us-west4"),
        (
            "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
            "111-browser.apps.googleusercontent.com",
        ),
        (
            "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI",
            "222-gemini.apps.googleusercontent.com",
        ),
    ];

    /// A getter over an in-memory `(key, value)` slice — the test analogue
    /// of `std::env::var`, so the substitution resolver is exercised without
    /// mutating the process environment.
    fn env_getter(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    /// The placeholder tokens that must be gone from every rendered file.
    ///
    /// Enumerating them catches a substitution that stopped firing, but it
    /// cannot catch a placeholder nobody added here —
    /// [`surviving_your_tokens`] covers that half.
    const PLACEHOLDER_TOKENS: &[&str] = &[
        "YOUR_PROJECT_ID",
        IMAGE_REGISTRY_TOKEN,
        "YOUR_GCP_REGION",
        "NAVIGATOR_WEB_IMAGE",
        "NAVIGATOR_IMAGE_REGISTRY",
        "NAVIGATOR_PUBLIC_HOST",
        "NAVIGATOR_WORKFLOWS_HOST",
        "YOUR_DOCUMENTS_BUCKET",
        "YOUR_APPLICATIONS_BUCKET",
        "YOUR_ASSETS_BUCKET",
        "YOUR_ASSET_BASE_URL",
        "YOUR_EXPORTS_BUCKET",
        "NAVIGATOR_GATEWAY_IP_NAME",
        "NAVIGATOR_GCP_SERVICE_ACCOUNT_ID",
        "navigator-web-secrets",
        "YOUR_GOOGLE_OAUTH_REQUIRED_HD",
        "YOUR_OAUTH_CLIENT_ID_BROWSER",
        "YOUR_OAUTH_CLIENT_ID_GEMINI",
        "YOUR_CHATWOOT_WEBSITE_TOKEN",
        "YOUR_OAUTH_MICROSOFT_CLIENT_ID",
        "YOUR_OAUTH_MICROSOFT_ALLOWED_TENANTS",
        RELEASE_TAG_TOKEN,
    ];

    /// Every `YOUR_*` placeholder still present in `text`.
    ///
    /// Read out of the rendered text rather than compared against
    /// [`PLACEHOLDER_TOKENS`], because that list is hand-maintained and so can
    /// only report a token somebody remembered to add to it.
    /// `YOUR_RESTATE_CLOUD_INGRESS` sat in six shipped trigger `CronJob`s and
    /// on no substitution table at all: the enumerated check passed while every
    /// `ops ship` rendered a literal `https://YOUR_RESTATE_CLOUD_INGRESS` into
    /// the cluster. This finds the next one without being told its name.
    fn surviving_your_tokens(text: &str) -> Vec<String> {
        text.match_indices("YOUR_")
            .map(|(at, _)| {
                text[at..]
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                    .collect()
            })
            .collect()
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn render_substitutes_every_placeholder_to_zero_remaining() {
        // TDD step 1: given a full NAVIGATOR_* env, the rendered tree has
        // ZERO `YOUR_*` / `your-domain.example` placeholders. A leftover
        // token would ship a broken bucket name or cert domain.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");

        let gke = rendered.path().join(GKE_KUSTOMIZE_SUBPATH);
        assert!(
            gke.join("kustomization.yaml").is_file(),
            "the GKE kustomize root is rendered"
        );
        assert!(
            rendered
                .path()
                .join(K8S_BASE_SUBPATH)
                .join("kustomization.yaml")
                .is_file(),
            "the shared base is rendered alongside so `../../../../k8s/base` resolves"
        );
        assert!(
            rendered
                .path()
                .join(EXPORTS_KUSTOMIZE_SUBPATH)
                .join("kustomization.yaml")
                .is_file(),
            "the scheduled-job tree is rendered alongside so `../exports` resolves"
        );

        let mut files_seen = 0;
        for entry in walkdir::WalkDir::new(rendered.path()) {
            let entry = entry.expect("walk rendered tree");
            if !entry.file_type().is_file() {
                continue;
            }
            let text = fs::read_to_string(entry.path()).expect("read rendered file");
            for token in PLACEHOLDER_TOKENS {
                assert!(
                    !text.contains(token),
                    "{} still contains placeholder `{token}` after render",
                    entry.path().display()
                );
            }
            let surviving = surviving_your_tokens(&text);
            assert!(
                surviving.is_empty(),
                "{} still contains placeholder(s) {surviving:?} after render. A `YOUR_*` token \
                 that no substitution resolves reaches the cluster verbatim: either give it an \
                 entry in `resolve_substitutions_for_deployment`, or source the value from the \
                 deployment Secret instead of writing it inline",
                entry.path().display()
            );
            files_seen += 1;
        }
        assert!(files_seen > 0, "the render wrote at least one file");

        // The real values landed where the placeholders were.
        let web_env = fs::read_to_string(gke.join("patches/web-env.yaml")).unwrap();
        assert!(
            web_env.contains("neon-production-assets"),
            "deployment-specific assets bucket substituted"
        );
        assert!(
            web_env.contains("neon-production-documents"),
            "deployment-specific documents bucket substituted"
        );
        assert!(
            web_env.contains("name: NAVIGATOR_DOCUMENTS_BUCKET"),
            "documents bucket environment-variable name is preserved"
        );
        assert!(
            web_env.contains("name: NAVIGATOR_ASSETS_BUCKET"),
            "assets bucket environment-variable name is preserved"
        );
        assert!(
            web_env.contains("name: NAVIGATOR_APPLICATIONS_BUCKET"),
            "applications bucket environment-variable name is preserved"
        );
        assert!(
            web_env.contains("neon-production-applications"),
            "deployment-specific applications bucket substituted"
        );
        // Every inline-env web key must appear as a `name:` here. `$patch:
        // replace` drops the KIND base env list, so any INLINE_ENV_WEB_KEYS
        // entry absent from this patch is a boot requirement the rolled pod
        // never receives — and `ops ship`'s preflight then aborts the roll.
        for key in INLINE_ENV_WEB_KEYS {
            assert!(
                web_env.contains(&format!("name: {key}")),
                "inline-env web key `{key}` is declared in the rendered GKE web env"
            );
        }
        // The public asset origin must reach the pod, not merely pass
        // `require_asset_base_url`'s config.toml check. `$patch: replace`
        // drops the base env list wholesale, so an entry missing here is an
        // entry the rolled binary never sees — it then resolves every
        // content image against the empty `/public` fallback and each hero
        // 404s while the bucket itself is perfectly healthy.
        assert!(
            web_env.contains("name: NAVIGATOR_ASSET_BASE_URL"),
            "asset base URL environment-variable name is preserved"
        );
        assert!(
            web_env.contains("https://www.neonlaw.com/assets"),
            "deployment-specific asset base URL substituted"
        );
        assert!(
            web_env.contains("name: GOOGLE_OAUTH_REQUIRED_HD"),
            "OAuth hosted-domain environment-variable name is preserved"
        );
        // The support-chat coordinate is optional, so the drift that matters is
        // the reverse of the required keys': the env *name* must survive even
        // when the value renders empty, or a deployment that later adopts the
        // widget writes a `config.toml` line the pod never receives.
        assert!(
            web_env.contains(&format!("name: {NAVIGATOR_CHATWOOT_WEBSITE_TOKEN}")),
            "support-chat environment-variable name is preserved"
        );
        // The break-glass Owner must reach the pod for the same reason the
        // asset origin must: `$patch: replace` drops the base env list, so an
        // entry missing here is one the rolled binary never sees — and a
        // deployment with no bootstrap Owner 403s every first sign-in with no
        // way to seed the first Person.
        assert!(
            web_env.contains("name: NAVIGATOR_BOOTSTRAP_OWNER_EMAIL"),
            "bootstrap Owner environment-variable name is preserved"
        );
        assert!(
            web_env.contains("value: nick@neonlaw.com"),
            "bootstrap Owner identity reaches the pod"
        );
        assert!(
            web_env.contains("111-browser.apps.googleusercontent.com"),
            "browser OAuth client id substituted (no doubled suffix)"
        );
        assert!(
            web_env.contains("222-gemini.apps.googleusercontent.com"),
            "gemini OAuth client id substituted"
        );
        assert!(
            web_env.contains("https://www.neonlaw.com/auth/callback"),
            "public host substituted into the redirect URI"
        );
        assert!(
            web_env.contains("value: neonlaw.com"),
            "OAuth hosted-domain restriction substituted"
        );
        assert!(
            web_env.contains("namespace: neon-production"),
            "deployment-specific namespace substituted"
        );
        let workflows = fs::read_to_string(gke.join("workflows-service/deployment.yaml")).unwrap();
        assert!(
            workflows.contains("value: neon-production-exports"),
            "worker receives the deployment-specific exports bucket"
        );
        assert!(
            !workflows.contains("neon-law-420305-exports"),
            "worker must not derive a shared exports bucket from the GCP project"
        );
        let namespace_manifest = fs::read_to_string(
            rendered
                .path()
                .join(K8S_BASE_SUBPATH)
                .join("namespace.yaml"),
        )
        .unwrap();
        assert!(
            namespace_manifest.contains("name: neon-production"),
            "the Namespace object itself uses the deployment namespace"
        );
        assert!(!namespace_manifest.contains("name: navigator"));
    }

    /// Collect every `env:` entry declared anywhere under `node`, as
    /// `(name, entry)`. Recursive rather than pointer-indexed because the
    /// scheduled jobs nest their pod spec differently from a Deployment
    /// (`spec.jobTemplate.spec.template.spec.containers`), and a check that
    /// hard-codes one shape silently stops looking when the other appears.
    fn env_entries(node: &serde_json::Value, found: &mut Vec<(String, serde_json::Value)>) {
        match node {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::Array(env)) = map.get("env") {
                    for entry in env {
                        if let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) {
                            found.push((name.to_string(), entry.clone()));
                        }
                    }
                }
                for value in map.values() {
                    env_entries(value, found);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    env_entries(item, found);
                }
            }
            _ => {}
        }
    }

    /// A key the `SecretProviderClass` projects must reach a scheduled trigger
    /// FROM that Secret, never as an inline literal.
    ///
    /// `ops ship` applies these `CronJob`s over the live objects, and a
    /// container's `env` list merges by entry name. An inline `value` here
    /// therefore merges with the cluster's `valueFrom` into a single entry
    /// carrying both, which the API server rejects outright — "may not be
    /// specified when `value` is not empty". That aborted `kubectl diff -k`
    /// before it compared anything, so for two releases no manifest change of
    /// any kind could reach either cluster and version rolls were stuck on
    /// `--image-only`. The projected set is read from the class itself, so the
    /// next trigger that inlines a projected key fails here without this test
    /// being updated.
    ///
    /// Deliberately scoped to the exports tree. `k8s/base/web/web.yaml` sets
    /// several projected keys inline as KIND development values, and the GKE
    /// overlay's `patches/web-env.yaml` `$patch: replace`s that whole list
    /// away. No patch rewrites these `CronJob`s, so what the file says is what
    /// ships.
    #[test]
    fn triggers_source_every_projected_key_from_the_deployment_secret() {
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let projected = secret_provider_class_keys();
        // The render substitutes the Secret's name per deployment, so the
        // reference has to follow it — a trigger pinned to the literal
        // `navigator-web-secrets` would read another deployment's Secret, or
        // more likely none at all.
        let secret_name = FULL_ENV
            .iter()
            .find(|(key, _)| *key == "NAVIGATOR_WEB_SECRET_NAME")
            .map(|(_, value)| *value)
            .expect("the render env names the projected Secret");

        let mut sourced_from_secret = 0;
        for entry in walkdir::WalkDir::new(rendered.path().join(EXPORTS_KUSTOMIZE_SUBPATH)) {
            let entry = entry.expect("walk the rendered exports tree");
            if !entry.file_type().is_file() {
                continue;
            }
            let text = fs::read_to_string(entry.path()).expect("read rendered manifest");
            let manifest: serde_json::Value =
                serde_yaml::from_str(&text).expect("rendered scheduled-job manifest parses");
            let mut env = Vec::new();
            env_entries(&manifest, &mut env);
            let file = entry.path().display();
            for (name, declared) in env {
                if !projected.contains(&name) {
                    continue;
                }
                assert!(
                    declared.get("value").is_none(),
                    "{file} sets `{name}` — a key the SecretProviderClass projects — as an inline \
                     literal. Applying that over the live CronJob merges `value` and `valueFrom` \
                     into one env entry and the API server rejects the whole object, taking every \
                     manifest change down with it. Use \
                     `valueFrom.secretKeyRef` into the projected Secret."
                );
                let secret_ref = declared
                    .pointer("/valueFrom/secretKeyRef")
                    .unwrap_or_else(|| panic!("{file} sources `{name}` from a Secret"));
                assert_eq!(
                    secret_ref.get("name").and_then(serde_json::Value::as_str),
                    Some(secret_name),
                    "{file} must read `{name}` from this deployment's own projected Secret"
                );
                assert_eq!(
                    secret_ref.get("key").and_then(serde_json::Value::as_str),
                    Some(name.as_str()),
                    "{file} must read `{name}` from the Secret key of the same name"
                );
                // `optional: true` is load-bearing, not decoration.
                // `omit_unwritten_objects` drops an object the shipping
                // deployment does not write from the rendered class, so the
                // projected Secret of an ordinary row carries no
                // RESTATE_INGRESS_URL at all — the assertion below proves that
                // from the fixture tree. A required reference would then leave
                // every trigger pod in CreateContainerConfigError, never
                // started and never logging why.
                assert_eq!(
                    secret_ref
                        .get("optional")
                        .and_then(serde_json::Value::as_bool),
                    Some(true),
                    "{file} must mark its `{name}` reference optional, or a deployment that does \
                     not project the key gets pods that never start instead of a trigger that \
                     exits naming the missing value"
                );
                sourced_from_secret += 1;
            }
        }

        // The six trigger CronJobs each carry RESTATE_INGRESS_URL and
        // RESTATE_AUTH_TOKEN. A floor rather than an equality so a new trigger
        // does not fail this, while a file that quietly drops the reference —
        // the state that would make the loop above vacuous — does.
        assert!(
            sourced_from_secret >= 12,
            "expected at least the six triggers' two Restate keys to be sourced from the Secret, \
             saw {sourced_from_secret}"
        );

        // The premise behind `optional: true`, proven rather than asserted:
        // an ordinary deployment legitimately does not project this key.
        let ordinary = super::super::deployments::Deployment::load(&fixture_tree(), ORDINARY_ROW)
            .expect("the ordinary row loads");
        assert!(
            ordinary.provisioned,
            "{ORDINARY_ROW} must be provisioned or the omission below is computed from no \
             secrets file at all and this assertion proves nothing"
        );
        assert!(
            super::super::deployments::skipped_projected_objects(&ordinary)
                .expect("the tree is complete")
                .contains("RESTATE_INGRESS_URL"),
            "RESTATE_INGRESS_URL is scoped to the automation home, so an ordinary deployment's \
             Secret does not carry it — which is why the triggers' reference must be optional"
        );
    }

    #[test]
    fn web_pod_binds_the_navigator_web_service_account() {
        // Regression guard. The `navigator-web` Deployment MUST render with
        // `serviceAccountName: navigator-web` so the pod runs under Workload
        // Identity as the KSA-bound GSA (`navigator-web@…`, which holds the
        // object-storage and Secret Manager grants). The base
        // (`k8s/base/web/web.yaml`) omits it
        // because KIND has no such KSA, so the GKE overlay owns it. When it
        // went missing, `kubectl apply` pruned the field from the live
        // Deployment and the pod fell back to the `default` KSA, whose GSA
        // holds none of those grants. Assert the overlay patch pins it so a
        // future edit can't silently drop it again.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let gke = rendered.path().join(GKE_KUSTOMIZE_SUBPATH);

        // (1) The patch file pins the KSA on the pod spec.
        let sa_patch = fs::read_to_string(gke.join("patches/web-service-account.yaml")).unwrap();
        assert!(
            sa_patch.contains("serviceAccountName: navigator-web"),
            "the web overlay must bind the navigator-web KSA, or the pod \
             falls back to `default` and loses every Workload Identity grant"
        );

        // (2) …and the kustomization actually APPLIES that patch to the
        // navigator-web Deployment. Checking only (1) would still pass if the
        // patch were dropped from `kustomization.yaml`'s `patches:` list or
        // retargeted — the file would keep the line while `kubectl apply -k`
        // rendered a pod on the `default` KSA. Assert
        // the exact wiring block so removal or retarget fails the test too.
        let kustomization = fs::read_to_string(gke.join("kustomization.yaml")).unwrap();
        assert!(
            kustomization.contains(
                "path: patches/web-service-account.yaml\n    \
                 target:\n      kind: Deployment\n      name: navigator-web"
            ),
            "kustomization must apply web-service-account.yaml to the \
             navigator-web Deployment, or the pinned KSA never reaches the pod"
        );

        // (3) The real thing: build the overlay with `kubectl kustomize` — the
        // same kustomize `ops ship` runs via `kubectl -k` — and assert the
        // MERGED navigator-web Deployment carries the KSA. This proves the
        // strategic-merge patch actually lands, not just that the text is
        // present (Greptile P1/security on #395). The `cargo test (workspace)`
        // CI job ships no kubectl (see .github/workflows/ci.yml), so this is
        // skipped there — (1)+(2) still guard the realistic removal/retarget
        // edits; where kubectl exists (dev, the deploy job) it is authoritative.
        match kustomize_deployment_service_account(gke.as_path(), "navigator-web") {
            KustomizeSa::ServiceAccount(sa) => assert_eq!(
                sa.as_deref(),
                Some("navigator-web"),
                "kustomize-rendered navigator-web Deployment must set \
                 serviceAccountName: navigator-web"
            ),
            KustomizeSa::KubectlUnavailable => eprintln!(
                "kubectl not available — skipping the kustomize-render assertion; \
                 wiring + patch-text assertions still ran"
            ),
        }
    }
    /// Outcome of reading a Deployment's KSA from a `kubectl kustomize` build.
    enum KustomizeSa {
        /// kubectl is absent or the build failed — the caller skips the render
        /// assertion (the `cargo test (workspace)` CI job has no kubectl; the
        /// text + wiring assertions still guard there).
        KubectlUnavailable,
        /// The named Deployment's `serviceAccountName` (`None` = unset).
        ServiceAccount(Option<String>),
    }

    /// Build the rendered GKE overlay with `kubectl kustomize` (the kustomize
    /// engine `ops ship` drives through `kubectl -k`) and return the named
    /// Deployment's `.spec.template.spec.serviceAccountName`.
    fn kustomize_deployment_service_account(gke_dir: &Path, deployment: &str) -> KustomizeSa {
        use serde::Deserialize;
        let Ok(out) = Command::new("kubectl")
            .arg("kustomize")
            .arg(gke_dir)
            .output()
        else {
            return KustomizeSa::KubectlUnavailable;
        };
        if !out.status.success() {
            return KustomizeSa::KubectlUnavailable;
        }
        let Ok(stdout) = String::from_utf8(out.stdout) else {
            return KustomizeSa::KubectlUnavailable;
        };
        for document in serde_yaml::Deserializer::from_str(&stdout) {
            let Ok(value) = serde_yaml::Value::deserialize(document) else {
                continue;
            };
            let is_deployment =
                value.get("kind").and_then(serde_yaml::Value::as_str) == Some("Deployment");
            let name = value
                .get("metadata")
                .and_then(|m| m.get("name"))
                .and_then(serde_yaml::Value::as_str);
            if is_deployment && name == Some(deployment) {
                let sa = value
                    .get("spec")
                    .and_then(|s| s.get("template"))
                    .and_then(|t| t.get("spec"))
                    .and_then(|s| s.get("serviceAccountName"))
                    .and_then(serde_yaml::Value::as_str)
                    .map(str::to_string);
                return KustomizeSa::ServiceAccount(sa);
            }
        }
        KustomizeSa::ServiceAccount(None)
    }

    #[test]
    fn resolve_substitutions_fails_by_name_when_a_var_is_missing() {
        // TDD step 2: a missing (or blank) required substitution var → a
        // named, actionable error, before anything is written (no partial
        // apply). Assert each var is the one that trips the error.
        for missing in [
            "NAVIGATOR_GCP_PROJECT_ID",
            "NAVIGATOR_GCP_LOCATION",
            "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
        ] {
            // Present vars resolve to a value that also satisfies the OAuth
            // suffix check, so only the truly-missing var trips the error.
            let getter = |key: &str| {
                if key == missing {
                    None
                } else {
                    Some("val.apps.googleusercontent.com".to_string())
                }
            };
            let err = resolve_substitutions_for_deployment("neon-production", "26.7.15", getter)
                .expect_err("a missing var must fail the resolve")
                .to_string();
            assert!(
                err.contains(missing),
                "error must name the missing var `{missing}`, got: {err}"
            );
            assert!(
                err.contains("deployments/neon-production/config.toml"),
                "error must send the operator to the deployment's own config file, \
                 never a retired Doppler config: {err}"
            );
        }
    }

    #[test]
    fn resolve_substitutions_allows_gemini_to_remain_null_before_registration() {
        let getter = |key: &str| {
            if key == "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI" {
                None
            } else {
                env_getter(FULL_ENV)(key)
            }
        };
        let substitutions =
            resolve_substitutions_for_deployment("neon-production", "26.7.15", getter)
                .expect("browser-only OAuth configuration must ship");
        let browser = substitutions
            .iter()
            .find(|substitution| substitution.token == "YOUR_OAUTH_CLIENT_ID_BROWSER")
            .unwrap();
        let gemini = substitutions
            .iter()
            .find(|substitution| substitution.token == "YOUR_OAUTH_CLIENT_ID_GEMINI")
            .unwrap();

        assert_eq!(gemini.value, browser.value);
        assert_eq!(gemini.env, "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI");
    }

    #[test]
    fn resolve_substitutions_rejects_a_bare_oauth_client_id() {
        // A bare OAuth id (no `.apps.googleusercontent.com`) renders an
        // `OAUTH_CLIENT_ID` Google won't match, breaking login. The resolve
        // must refuse it by name rather than ship a broken redirect.
        for bare in [
            "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
            "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI",
        ] {
            let getter = |key: &str| {
                Some(if key == bare {
                    "1234567890-bareid".to_string() // no suffix
                } else if key.starts_with("NAVIGATOR_OAUTH_CLIENT_ID") {
                    "999-other.apps.googleusercontent.com".to_string()
                } else {
                    "some-value".to_string()
                })
            };
            let err = resolve_substitutions_for_deployment("neon-production", "26.7.15", getter)
                .expect_err("a bare OAuth id must fail the resolve")
                .to_string();
            assert!(
                err.contains(bare),
                "error names the offending var `{bare}`: {err}"
            );
            assert!(
                err.contains(GOOGLE_OAUTH_CLIENT_ID_SUFFIX),
                "error names the required suffix: {err}"
            );
        }

        // A value with the suffix somewhere in the MIDDLE (trailing garbage
        // after it) is also malformed — Google matches the client id
        // verbatim, so `123.apps.googleusercontent.com.extra` renders an
        // `OAUTH_CLIENT_ID` login rejects. The suffix must be at the END.
        for malformed in [
            "NAVIGATOR_OAUTH_CLIENT_ID_BROWSER",
            "NAVIGATOR_OAUTH_CLIENT_ID_GEMINI",
        ] {
            let getter = |key: &str| {
                Some(if key == malformed {
                    "123.apps.googleusercontent.com.extra".to_string() // suffix not at end
                } else if key.starts_with("NAVIGATOR_OAUTH_CLIENT_ID") {
                    "999-other.apps.googleusercontent.com".to_string()
                } else {
                    "some-value".to_string()
                })
            };
            let err = resolve_substitutions_for_deployment("neon-production", "26.7.15", getter)
                .expect_err("a client id with the suffix not at the end must fail")
                .to_string();
            assert!(
                err.contains(malformed),
                "error names the offending var `{malformed}`: {err}"
            );
        }

        // The full-id form (both OAuth vars carry the suffix) resolves fine.
        assert!(resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV)
        )
        .is_ok());
    }

    #[test]
    fn render_temp_dir_is_removed_when_dropped() {
        // TDD step 3: the rendered manifests live only for the span of the
        // `kubectl` calls — dropping the TempDir removes them (no leak).
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .unwrap();
        let rendered = render_manifests_with(&subs, false).unwrap();
        let path = rendered.path().to_path_buf();
        assert!(path.join(GKE_KUSTOMIZE_SUBPATH).exists());
        drop(rendered);
        assert!(
            !path.exists(),
            "the rendered manifest temp dir must be gone after drop"
        );
    }

    #[test]
    fn dry_run_reconcile_diffs_but_never_applies() {
        // TDD step 5: `--dry-run` renders + diffs, never applies.
        assert_eq!(reconcile_verbs(true), &["diff"]);
        assert!(
            !reconcile_verbs(true).contains(&"apply"),
            "dry-run must not apply"
        );
        // A live run diffs THEN applies.
        assert_eq!(reconcile_verbs(false), &["diff", "apply"]);

        // Drive the executor with a recorder (no cluster): dry-run issues
        // `diff` only; a live run issues `diff` then `apply`.
        let mut dry = Vec::new();
        reconcile_kustomize(true, |verb| {
            dry.push(verb.to_string());
            Ok(0)
        })
        .unwrap();
        assert_eq!(dry, vec!["diff"], "dry-run must not apply");

        let mut live = Vec::new();
        reconcile_kustomize(false, |verb| {
            live.push(verb.to_string());
            Ok(0)
        })
        .unwrap();
        assert_eq!(live, vec!["diff", "apply"]);
    }

    #[test]
    fn reconcile_diff_drift_is_benign_but_diff_errors_abort() {
        // exit 0 = no drift, exit 1 = drift (the normal signal) → both proceed.
        assert!(reconcile_kustomize(true, |_| Ok(0)).is_ok());
        assert!(reconcile_kustomize(true, |_| Ok(1)).is_ok());
        // exit >1 = a real diff error (bad context / auth / kustomize build) →
        // the mandatory drift-review gate never ran, so it aborts rather than
        // let a `--dry-run` exit 0 or a live run fall through to `apply`.
        let errored = reconcile_kustomize(true, |_| Ok(2));
        assert!(errored.is_err(), "a >1 diff exit must abort");
        assert!(
            errored.unwrap_err().to_string().contains("NOT checked"),
            "the abort names that the manifests were not checked"
        );
        // …as does a kubectl that cannot be spawned.
        let unspawnable = reconcile_kustomize(true, |_| -> Result<i32> { bail!("no kubectl") });
        assert!(unspawnable.is_err(), "an unspawnable diff must abort");
    }

    #[test]
    fn reconcile_apply_failure_aborts() {
        // A non-zero apply exit aborts the reconcile — a partial apply is
        // worse than none.
        let nonzero =
            reconcile_kustomize(false, |verb| if verb == "apply" { Ok(1) } else { Ok(0) });
        assert!(nonzero.is_err(), "a non-zero apply exit must abort");
        // As does an apply whose kubectl cannot be spawned.
        let unspawnable = reconcile_kustomize(false, |verb| -> Result<i32> {
            if verb == "apply" {
                bail!("boom")
            }
            Ok(0)
        });
        assert!(unspawnable.is_err(), "an unspawnable apply must abort");
    }

    #[test]
    fn reconcile_is_self_contained_needs_no_overlay_dir() {
        // TDD step 6: the reconcile is env-driven from the four substitution
        // vars only — NAVIGATOR_GKE_OVERLAY_DIR is retired and reading it is
        // gone. The render succeeds with only those vars set; there is no
        // overlay path in the ShipConfig or the substitution table.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .expect("no overlay dir required");
        assert!(
            subs.iter().all(|s| s.env != "NAVIGATOR_GKE_OVERLAY_DIR"),
            "the retired overlay-dir var is not part of the substitution table"
        );
        render_manifests_with(&subs, false).expect("render needs no overlay folder on disk");
    }

    #[test]
    fn apply_substitutions_replaces_all_tokens_in_one_pass() {
        let subs = vec![
            Substitution {
                token: "YOUR_PROJECT_ID",
                env: "NAVIGATOR_GCP_PROJECT_ID",
                value: "proj-1".into(),
            },
            Substitution {
                token: "your-domain.example",
                env: "brand.primary_domain",
                value: "example.org".into(),
            },
        ];
        let out = apply_substitutions(
            "bucket: YOUR_PROJECT_ID-assets host: www.your-domain.example YOUR_PROJECT_ID",
            &subs,
        );
        assert_eq!(out, "bucket: proj-1-assets host: www.example.org proj-1");
    }

    #[test]
    fn asset_base_url_present_requires_a_non_blank_origin() {
        // The ship gate treats unset and blank identically: both mean the
        // rolled site would resolve images against an empty `/public`.
        assert!(!asset_base_url_present(None));
        assert!(!asset_base_url_present(Some("")));
        assert!(!asset_base_url_present(Some("   ")));
        assert!(asset_base_url_present(Some(
            "https://storage.googleapis.com/my-org-prod-assets"
        )));
    }

    #[test]
    fn require_asset_base_url_value_names_the_key_and_the_config_file_when_absent() {
        assert!(require_asset_base_url_value(
            "example-prod",
            Some("https://storage.googleapis.com/my-org-prod-assets")
        )
        .is_ok());
        let err = require_asset_base_url_value("example-prod", None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(ASSET_BASE_URL_KEY),
            "names the key, got: {err}"
        );
        assert!(
            err.contains("deployments/example-prod/config.toml"),
            "points at the deployment's own config file, got: {err}"
        );
        assert!(
            err.contains("ops ship --deployment example-prod"),
            "the re-run line carries the explicit deployment flag, got: {err}"
        );
    }

    #[test]
    fn asset_bucket_preflight_uses_the_selected_deployments_coordinate() {
        let mut checked = None;
        verify_assets_bucket_value_with(
            "example-prod",
            Some("  example-prod-assets  "),
            |bucket| {
                checked = Some(bucket.to_string());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(checked.as_deref(), Some("example-prod-assets"));

        let error = verify_assets_bucket_value_with("example-prod", None, |_| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(error.contains(ASSETS_BUCKET_KEY), "got: {error}");
        assert!(
            error.contains("deployments/example-prod/config.toml"),
            "got: {error}"
        );
    }

    /// The preflight reads the bucket from the selected deployment's own
    /// coordinates, so a row that never declared one stops the ship before any
    /// storage client is opened.
    #[test]
    fn asset_bucket_preflight_stops_a_deployment_with_no_bucket_coordinate() {
        let deployment = super::super::deployments::Deployment {
            name: "example-prod".to_string(),
            kms_key: "projects/my-org-prod/locations/us-west4/keyRings/sops/cryptoKeys/navigator"
                .to_string(),
            provisioned: true,
            coordinates: std::collections::BTreeMap::new(),
            encrypted_keys: std::collections::BTreeSet::new(),
        };

        let error = verify_assets_bucket(&deployment)
            .expect_err("a deployment with no assets bucket cannot be preflighted")
            .to_string();

        assert!(error.contains(ASSETS_BUCKET_KEY), "got: {error}");
        assert!(
            error.contains("deployments/example-prod/config.toml"),
            "got: {error}"
        );
    }

    /// The keystone of the no-fallback design: every deployment in the tree
    /// resolves to a complete, self-consistent `ShipConfig` with no process
    /// environment at all. A deployment this test passes for is one
    /// `ops ship --deployment <name>` can at least *begin* to roll — dropping
    /// a new `deployments/<name>/` pair into the tree is the whole activation.
    ///
    /// The tree it reads is the synthetic one; the real rows are held to the
    /// same assertion by `navigator ops deployments check`.
    #[test]
    fn every_checked_in_deployment_resolves_a_complete_ship_config() {
        let root = fixture_tree();
        for name in super::super::deployments::names(&root).expect("the tree is readable") {
            let deployment = super::super::deployments::Deployment::load(&root, &name)
                .expect("the deployment loads");
            assert!(
                deployment.provisioned,
                "{name} must be provisioned or this assertion is dormant"
            );
            let cfg = ShipConfig::from_deployment(&deployment)
                .unwrap_or_else(|error| panic!("{name} must resolve a ShipConfig: {error}"));
            assert_eq!(cfg.name, name);
            assert_eq!(
                check_context(&cfg.context, &cfg.project_id, &cfg.location, &cfg.cluster),
                ContextCheck::Matches,
                "{name}'s NAVIGATOR_GKE_CONTEXT must name its own cluster"
            );
            require_asset_base_url(&deployment)
                .unwrap_or_else(|error| panic!("{name} must carry its asset origin: {error}"));
            resolve_substitutions_for_deployment(&name, "26.1.1", |key| {
                deployment.coordinates.get(key).cloned()
            })
            .unwrap_or_else(|error| {
                panic!("{name} must resolve every manifest substitution: {error}")
            });
        }
    }

    #[test]
    fn a_missing_coordinate_names_the_deployments_config_file() {
        let err = ShipConfig::from_lookup("example-prod", |key| {
            (key == "NAVIGATOR_ENVIRONMENT").then(|| "production".to_string())
        })
        .expect_err("an empty coordinate map cannot resolve a ShipConfig")
        .to_string();
        assert!(
            err.contains("deployments/example-prod/config.toml"),
            "the first missing coordinate must name the file to fix: {err}"
        );
    }

    #[test]
    fn derived_names_target_the_published_registry() {
        let cfg = sample_config();
        assert_eq!(cfg.registry(), "ghcr.io/neon-law-source-code");
        assert_eq!(
            cfg.web_image("26.6.23"),
            "ghcr.io/neon-law-source-code/neon-server:26.6.23"
        );
        assert_eq!(
            cfg.workflows_image("26.6.23"),
            "ghcr.io/neon-law-source-code/navigator-workflows-service:26.6.23"
        );
    }

    #[test]
    fn every_deployment_pulls_the_same_published_images() {
        // Both rows resolve to one namespace, which is what makes staging a
        // proving ring rather than a different build: production rolls the
        // exact image staging has been serving. The GAR shape needed a region,
        // a hub project, and a repository name to agree before that held; this
        // one cannot disagree with itself.
        assert_eq!(images_registry(None), "ghcr.io/neon-law-source-code");
        assert_eq!(
            images_registry(Some("ghcr.io/neon-law-source-code")),
            "ghcr.io/neon-law-source-code"
        );
    }

    #[test]
    fn a_fork_publishes_and_pulls_from_its_own_namespace() {
        // A fork sets one variable and every image reference follows it.
        assert_eq!(images_registry(Some("ghcr.io/acme")), "ghcr.io/acme");
    }

    #[test]
    fn ship_rolls_exactly_the_two_service_deployments() {
        // The single-writer git tier retired with the topology (#295): a ship
        // restarts and waits on `web` and the worker, and nothing conditional
        // on a third Deployment being installed.
        assert_eq!(
            restart_deployments(),
            ["navigator-web", "workflows-service"]
        );
        assert_eq!(
            rollout_wait_deployments(),
            ["navigator-web", "workflows-service"]
        );
        assert_eq!(web_binary_deployments(), ["navigator-web"]);
    }

    #[test]
    fn workflows_worker_registration_is_required() {
        // The folded-in GitHub webhook notice services (`DevxIssueTriage`,
        // `devx-pr`) register with `workflows-service`, so a failed re-register
        // must fail the ship — a stale handler list would silently drop webhook
        // submissions until a later successful registration.
        let targets = reregister_targets(&sample_config());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, WORKFLOWS_DEPLOYMENT);
        let err = apply_registration_result(
            &targets[0],
            Err(anyhow::anyhow!("registration unavailable")),
        )
        .expect_err("a registration failure must abort the ship");
        assert!(err.to_string().contains(WORKFLOWS_DEPLOYMENT));
    }

    #[test]
    fn render_pins_every_navigator_image_to_the_rolled_tag() {
        // The regression that wedged the 26.7.15 ship: the reconcile applied
        // `:YY.M.D` — never a published tag — and left a later `set image` to
        // fix it up. `maxSurge: 0` (workflows-service) deletes the running
        // pod on that first write, so the gap is a real outage on EVERY ship,
        // not just a failed one. The apply must land the real tag itself.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let gke = rendered.path().join(GKE_KUSTOMIZE_SUBPATH);

        let mut images = Vec::new();
        for entry in walkdir::WalkDir::new(rendered.path()) {
            let entry = entry.expect("walk rendered tree");
            if !entry.file_type().is_file() {
                continue;
            }
            let text = fs::read_to_string(entry.path()).expect("read rendered file");
            for line in text.lines() {
                let line = line.trim();
                if let Some(image) = line.strip_prefix("image: ") {
                    if image.contains("/navigator-") || image.contains("/neon-server:") {
                        images.push(image.to_string());
                    }
                }
            }
        }
        assert!(
            images
                .iter()
                .any(|image| image.contains("navigator-surreal-archive")),
            "the operational archive CronJob must be rendered: {images:?}"
        );
        for image in &images {
            assert!(
                image.ends_with(":26.7.15"),
                "`{image}` must be pinned to the rolled tag by the render itself"
            );
        }
        // Spot-check the file the incident traced to.
        let web_image = fs::read_to_string(gke.join("patches/web-image.yaml")).unwrap();
        assert!(web_image.contains("neon-server:26.7.15"));
        assert!(!web_image.contains(RELEASE_TAG_TOKEN));
    }

    #[test]
    fn images_render_from_the_registry_while_everything_else_renders_the_environment() {
        // The two projects are different and both must land: every image line
        // points at the hub CI publishes to, while the buckets and the GSA
        // stay in the project being shipped into.
        // Rendering images from the environment project applies a manifest
        // whose tags do not exist — an outage on the tiers that delete the
        // running pod before the replacement is ready.
        let subs =
            resolve_substitutions_for_deployment("neon-law-stg", "26.7.28", env_getter(HUB_ENV))
                .expect("hub env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");

        let mut images = Vec::new();
        let mut saw_environment_project = false;
        for entry in walkdir::WalkDir::new(rendered.path()) {
            let entry = entry.expect("walk rendered tree");
            if !entry.file_type().is_file() {
                continue;
            }
            let text = fs::read_to_string(entry.path()).expect("read rendered file");
            if text.contains("neon-law-stg") {
                saw_environment_project = true;
            }
            for line in text.lines() {
                if let Some(image) = line.trim().strip_prefix("image: ") {
                    // Only our own images. A rendered tree also carries
                    // third-party ones (Restate, ClamAV), which no substitution
                    // touches and which must not be held to our registry.
                    // Only our own images, and only the ones a substitution
                    // actually resolved. A rendered tree also carries
                    // third-party images (Restate, ClamAV) and the KIND base's
                    // registry-less `navigator-web:dev`, neither of which the
                    // registry rule governs.
                    if image.contains('/')
                        && (image.contains("navigator-") || image.contains("neon-server"))
                    {
                        images.push(image.to_string());
                    }
                }
            }
        }

        assert!(
            images
                .iter()
                .any(|image| image.contains("navigator-surreal-archive")),
            "the operational archive CronJob must render from the registry: {images:?}"
        );
        for image in &images {
            assert!(
                image.starts_with("ghcr.io/neon-law-source-code/"),
                "`{image}` must pull from the published registry"
            );
        }
        assert!(
            saw_environment_project,
            "the environment project must still render into buckets / the GSA"
        );
    }

    #[test]
    fn workflows_url_derives_from_primary_domain_when_unset() {
        // The 2026-06-10 ship symptom: a domain is configured but the
        // explicit URL is not. The resolved URL must be the real
        // ingress derived from the domain, never the
        // `workflows.example.com` placeholder.
        let cfg = ShipConfig {
            primary_domain: "neonlaw.com".into(),
            workflows_url: None,
            ..sample_config()
        };
        assert_eq!(
            cfg.workflows_url_resolved(),
            "https://workflows.neonlaw.com/"
        );
    }

    #[test]
    fn workflows_url_prefers_explicit_override() {
        let cfg = ShipConfig {
            workflows_url: Some("https://workflows.neonlaw.com/".into()),
            ..sample_config()
        };
        assert_eq!(
            cfg.workflows_url_resolved(),
            "https://workflows.neonlaw.com/"
        );
    }

    #[test]
    fn derived_context_matches_gke_get_credentials_naming() {
        assert_eq!(
            derived_context("my-org-prod", "us-west4", "navigator"),
            "gke_my-org-prod_us-west4_navigator"
        );
    }

    /// Shorthand for the expected parse: alternatives of conjunctions,
    /// no trigger.
    fn req(any_of: &[&[&str]]) -> SecretRequirement {
        SecretRequirement {
            any_of: any_of
                .iter()
                .map(|alt| alt.iter().map(ToString::to_string).collect())
                .collect(),
            trigger: None,
        }
    }

    #[test]
    fn secret_requirements_scrape_the_invariant_literals() {
        // Mirrors the real shape of web/src/config.rs invariant lines,
        // including a multi-line string and a prose false-positive.
        let src = r#"
            bail!(
                "RESTATE_BROKER_URL must be set (otherwise the in-memory \
                 broker would silently swallow jobs)"
            );
            ensure!(cfg.has("DOCUSIGN_HMAC_KEY"), "DOCUSIGN_HMAC_KEY must be set (otherwise forgeable)");
            // a comment explaining that something must be set should NOT match
            "SENDGRID_API_KEY must be set";
        "#;
        assert_eq!(
            secret_requirements(src),
            vec![
                req(&[&["DOCUSIGN_HMAC_KEY"]]),
                req(&[&["RESTATE_BROKER_URL"]]),
                req(&[&["SENDGRID_API_KEY"]]),
            ]
        );
    }

    #[test]
    fn disjunctive_invariants_parse_into_alternatives_of_conjunctions() {
        // The live DocuSign shape: `A or B + C + D must be set` — boots on
        // the short-lived access token alone, or on the full JWT triple
        // together. The chain must open the string literal; the same words in
        // prose (no quote before the first key) must not parse.
        let src = r#"
            "DOCUSIGN_ACCESS_TOKEN or DOCUSIGN_INTEGRATION_KEY + DOCUSIGN_USER_ID + DOCUSIGN_PRIVATE_KEY must be set \
             (prefer the JWT grant over the short-lived token)";
            // prose: DOCUSIGN_ACCESS_TOKEN or DOCUSIGN_INTEGRATION_KEY must be set somewhere
        "#;
        let parsed = secret_requirements(src);
        assert_eq!(
            parsed,
            vec![req(&[
                &["DOCUSIGN_ACCESS_TOKEN"],
                &[
                    "DOCUSIGN_INTEGRATION_KEY",
                    "DOCUSIGN_USER_ID",
                    "DOCUSIGN_PRIVATE_KEY"
                ],
            ])]
        );

        // Satisfied by the access-token alternative alone…
        let token: BTreeSet<String> = ["DOCUSIGN_ACCESS_TOKEN"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(parsed[0].is_satisfied_by(&token));
        // …or by the full JWT triple…
        let jwt: BTreeSet<String> = [
            "DOCUSIGN_INTEGRATION_KEY",
            "DOCUSIGN_USER_ID",
            "DOCUSIGN_PRIVATE_KEY",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert!(parsed[0].is_satisfied_by(&jwt));
        // …but NOT by a partial JWT triple, which is the failure mode a
        // key-set preflight exists to catch.
        let partial: BTreeSet<String> = ["DOCUSIGN_INTEGRATION_KEY", "DOCUSIGN_USER_ID"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(!parsed[0].is_satisfied_by(&partial));
        assert_eq!(
            unsatisfied_requirements(&parsed, &partial),
            parsed,
            "an incomplete alternative must leave the requirement unsatisfied"
        );
    }

    #[test]
    fn conditional_invariants_carry_their_trigger_and_gate_on_it() {
        // Mirrors the real shape: two conditional OIDC invariants nested
        // under an `if get("OIDC_JWKS_URL")` block, plus one unconditional.
        let src = r#"
            if get("OIDC_JWKS_URL").is_some_and(|s| !s.is_empty()) {
                violations.push(
                    "OIDC_AUDIENCE must be set when OIDC_JWKS_URL is (otherwise \
                     bearer tokens are accepted without audience pinning)"
                        .into(),
                );
                violations.push(
                    "OIDC_ISSUER must be set when OIDC_JWKS_URL is (otherwise the \
                     bearer token's issuer is unverified)"
                        .into(),
                );
            }
            "SENDGRID_API_KEY must be set (otherwise outbound email is dropped)";
        "#;
        let parsed = secret_requirements(src);
        assert_eq!(
            parsed,
            vec![
                SecretRequirement {
                    trigger: Some("OIDC_JWKS_URL".to_string()),
                    ..req(&[&["OIDC_AUDIENCE"]])
                },
                SecretRequirement {
                    trigger: Some("OIDC_JWKS_URL".to_string()),
                    ..req(&[&["OIDC_ISSUER"]])
                },
                req(&[&["SENDGRID_API_KEY"]]),
            ]
        );

        // OIDC_JWKS_URL not configured → the two conditional requirements
        // do NOT apply; only the unconditional one does. This is the prod
        // case where the optional JWKS bearer path is off.
        let without_jwks: BTreeSet<String> =
            ["SENDGRID_API_KEY"].into_iter().map(String::from).collect();
        assert_eq!(
            effective_requirements(&parsed, &without_jwks),
            vec![req(&[&["SENDGRID_API_KEY"]])]
        );
        assert!(unsatisfied_requirements(
            &effective_requirements(&parsed, &without_jwks),
            &without_jwks
        )
        .is_empty());

        // OIDC_JWKS_URL configured but its companions absent → both
        // conditional requirements now apply and are reported unsatisfied.
        let with_jwks: BTreeSet<String> = ["SENDGRID_API_KEY", "OIDC_JWKS_URL"]
            .into_iter()
            .map(String::from)
            .collect();
        let unsatisfied: Vec<String> =
            unsatisfied_requirements(&effective_requirements(&parsed, &with_jwks), &with_jwks)
                .iter()
                .map(SecretRequirement::describe)
                .collect();
        assert_eq!(
            unsatisfied,
            vec!["OIDC_AUDIENCE".to_string(), "OIDC_ISSUER".to_string()]
        );
    }

    #[test]
    fn ship_consumes_the_shared_web_invariant_keys() {
        let parsed = shared_web_requirements("my-org-prod");
        for expected in [
            "RESTATE_BROKER_URL",
            "SENDGRID_API_KEY",
            "DOCUSIGN_HMAC_KEY",
            "SESSION_SECRET",
        ] {
            assert!(
                parsed.iter().any(|requirement| requirement
                    .any_of
                    .iter()
                    .any(|alternative| alternative.iter().any(|key| key == expected))),
                "embedded config invariants missing {expected}; parsed: {parsed:?}"
            );
        }

        // Key presence alone is not enough: `ship` has to carry each
        // requirement's trigger across the parse too. If it dropped one, the
        // preflight would demand DocuSign credentials from a deployment that
        // declares none — the exact failure this gate exists to prevent, and
        // invisible to a check that only looks for the key.
        let docusign = parsed
            .iter()
            .find(|requirement| requirement.any_of == vec![vec!["DOCUSIGN_HMAC_KEY".to_string()]])
            .expect("DOCUSIGN_HMAC_KEY is a parsed requirement");
        assert_eq!(
            docusign.trigger.as_deref(),
            Some("DOCUSIGN_BASE_URL"),
            "the DocuSign trigger must survive the parse: {docusign:?}"
        );
    }

    #[test]
    fn ship_requires_receiver_credentials_only_for_the_automation_home() {
        let receiver = req(&[&[
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
            "NAVIGATOR_GITHUB_APP_LOGIN",
            "RESTATE_INGRESS_URL",
            "RESTATE_AUTH_TOKEN",
        ]]);
        assert!(!shared_web_requirements("neon-law").contains(&receiver));
        assert!(
            shared_web_requirements(store::deployment::GITHUB_AUTOMATION_HOME_PROJECT)
                .contains(&receiver)
        );
    }

    #[test]
    fn secret_requirements_dedupe() {
        let src = r#""SENDGRID_API_KEY must be set"; "SENDGRID_API_KEY must be set again";"#;
        assert_eq!(
            secret_requirements(src),
            vec![req(&[&["SENDGRID_API_KEY"]])]
        );
    }

    #[test]
    fn secret_requirements_collapse_conditional_and_unconditional_to_unconditional() {
        // A key required both conditionally (`… when X is`) and
        // unconditionally is ALWAYS required: the unconditional occurrence
        // wins and the merged requirement carries no trigger — regardless of
        // which occurrence the scraper meets first. Guards the `and_modify`
        // collapse in `secret_requirements`; a regression would gate an
        // always-required secret behind a trigger and let a missing key
        // slip past the preflight into a crash-loop.
        let want = vec![req(&[&["SENDGRID_API_KEY"]])];

        // Conditional first, then unconditional → trigger cleared.
        let cond_then_uncond = r#"
            "SENDGRID_API_KEY must be set when OIDC_JWKS_URL is (otherwise dropped)";
            "SENDGRID_API_KEY must be set (otherwise outbound email is dropped)";
        "#;
        assert_eq!(
            secret_requirements(cond_then_uncond),
            want,
            "unconditional occurrence must clear the earlier trigger"
        );

        // Unconditional first, then conditional → still unconditional.
        let uncond_then_cond = r#"
            "SENDGRID_API_KEY must be set (otherwise outbound email is dropped)";
            "SENDGRID_API_KEY must be set when OIDC_JWKS_URL is (otherwise dropped)";
        "#;
        assert_eq!(
            secret_requirements(uncond_then_cond),
            want,
            "a later conditional occurrence must not re-introduce a trigger"
        );
    }

    #[test]
    fn unsatisfied_requirements_reports_only_the_unsatisfied() {
        let required = vec![
            req(&[&["DOCUSIGN_HMAC_KEY"]]),
            req(&[&["SENDGRID_API_KEY"]]),
        ];
        let satisfied: BTreeSet<String> =
            ["SENDGRID_API_KEY"].into_iter().map(String::from).collect();
        assert_eq!(
            unsatisfied_requirements(&required, &satisfied),
            vec![req(&[&["DOCUSIGN_HMAC_KEY"]])]
        );
    }

    #[test]
    fn unsatisfied_requirements_empty_when_all_satisfied() {
        let required = vec![req(&[&["A"]]), req(&[&["B"]])];
        let satisfied: BTreeSet<String> = ["A", "B", "C"].into_iter().map(String::from).collect();
        assert!(unsatisfied_requirements(&required, &satisfied).is_empty());
    }

    #[test]
    fn describe_restates_the_invariant_phrasing() {
        let disjunction = req(&[&["A"], &["B", "C"]]);
        assert_eq!(disjunction.describe(), "A or B + C");
        assert_eq!(req(&[&["A"]]).describe(), "A");
    }

    /// A rendered manifest stream shaped like `kubectl kustomize` output:
    /// multiple documents, the web tier plus a non-web-binary workload.
    const RENDERED_MANIFESTS: &str = r"
apiVersion: v1
kind: Service
metadata:
  name: navigator-web
  namespace: navigator
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: navigator-web
  namespace: navigator
spec:
  template:
    spec:
      containers:
        - name: web
          env:
            - name: NAVIGATOR_BLANK
              value: ''
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: workflows-service
  namespace: navigator
spec:
  template:
    spec:
      containers:
        - name: worker
          env:
            - name: NAVIGATOR_WORKER_ONLY
              value: yes-please
";

    #[test]
    fn rendered_envs_read_the_web_binary_deployments_from_the_manifests() {
        let envs = parse_web_binary_envs(
            RENDERED_MANIFESTS,
            "navigator-web-secrets",
            &BTreeSet::new(),
        )
        .expect("parse rendered manifests");
        let names: Vec<&str> = envs.iter().map(|(name, _)| name.as_str()).collect();
        // `workflows-service` runs the worker binary, so it is not subject
        // to the web boot invariants and must not be checked against them.
        assert_eq!(names, vec!["navigator-web"]);

        let web = &envs[0].1;
        // An empty-valued env var is as unset as a missing one, exactly as
        // the pod's own `enforce_deployment_invariants` sees it.
        assert!(!web.contains("NAVIGATOR_BLANK"));
    }

    /// The Secret keys prod actually carries for the requirements the
    /// manifest fixture does not supply as env. Enough to satisfy every
    /// `WEB_REQUIREMENTS` entry that isn't manifest-provided.
    fn satisfying_secret_keys() -> BTreeSet<String> {
        store::deployment::WEB_REQUIREMENTS
            .iter()
            .filter_map(|r| r.any_of.first())
            .flat_map(|alt| alt.iter().map(ToString::to_string))
            .collect()
    }

    #[test]
    fn the_rendered_secret_provider_class_is_scoped_to_one_deployment() {
        // The precondition for activating the CSI sync (#594). Six deployments
        // across four projects share this one embedded manifest, so every
        // coordinate in it must come out of the render as the *selected*
        // deployment's own. A literal left behind here would point another
        // deployment's pods at this project's Secret Manager, or project into
        // a Secret name its Deployment does not read — a mount that succeeds
        // and a pod that crash-loops on its boot invariants.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.29",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let spc = fs::read_to_string(
            rendered
                .path()
                .join(GKE_KUSTOMIZE_SUBPATH)
                .join("secrets/secret-provider-class.yaml"),
        )
        .expect("read the rendered SecretProviderClass");

        // Every Secret Manager reference resolves in the deployment's project.
        assert!(
            spc.contains("projects/neon-law-420305/secrets/SESSION_SECRET/versions/latest"),
            "resourceName must name the deployment's own project"
        );
        assert!(
            !spc.contains("YOUR_PROJECT_ID"),
            "no unsubstituted project placeholder may survive the render"
        );

        // It projects into the Secret this deployment's workloads `envFrom`.
        assert!(
            spc.contains("secretName: neon-production-web-secrets"),
            "the projected Secret must be the deployment's own, not a shared default"
        );
        // …and lands in the deployment's namespace, not the shared default.
        assert!(spc.contains("namespace: neon-production"));

        // The mount half must agree, or the driver never reconciles the Secret.
        let mount = fs::read_to_string(
            rendered
                .path()
                .join(GKE_KUSTOMIZE_SUBPATH)
                .join("secrets/web-secrets-csi-mount.yaml"),
        )
        .expect("read the rendered CSI mount patch");
        assert!(mount.contains("namespace: neon-production"));
        assert!(
            mount.contains("secretProviderClass: navigator-web"),
            "the mount must name the SecretProviderClass this manifest declares"
        );
    }

    #[test]
    fn two_deployments_render_disjoint_secret_manager_coordinates() {
        // Same manifest, two configs: nothing may be shared between them.
        let render_for = |env| {
            let subs =
                resolve_substitutions_for_deployment("neon-production", "26.7.29", env_getter(env))
                    .expect("env resolves");
            let rendered = render_manifests_with(&subs, false).expect("render succeeds");
            fs::read_to_string(
                rendered
                    .path()
                    .join(GKE_KUSTOMIZE_SUBPATH)
                    .join("secrets/secret-provider-class.yaml"),
            )
            .expect("read the rendered SecretProviderClass")
        };

        let neon = render_for(FULL_ENV);
        let staging = render_for(HUB_ENV);
        assert!(neon.contains("secretName: neon-production-web-secrets"));
        assert!(staging.contains("secretName: neon-law-stg-web-secrets"));
        assert!(
            !staging.contains("neon-law-420305"),
            "one deployment's render must not carry another's project"
        );
        assert!(
            !neon.contains("neon-law-stg-web-secrets"),
            "one deployment's render must not carry another's Secret name"
        );
    }

    #[test]
    fn spc_projected_keys_ignores_a_key_with_no_source_path() {
        // A `secretObjects` key whose `objectName` has no matching
        // `parameters.secrets` path cannot be sourced by CSI, so it must not
        // count as projected — otherwise the guard passes while the pod boots
        // without that key.
        let drifted = r#"
apiVersion: secrets-store.csi.x-k8s.io/v1
kind: SecretProviderClass
spec:
  parameters:
    secrets: |
      - resourceName: "projects/x/secrets/SESSION_SECRET/versions/latest"
        path: "SESSION_SECRET"
  secretObjects:
    - secretName: navigator-web-secrets
      data:
        - objectName: "SESSION_SECRET"
          key: "SESSION_SECRET"
        - objectName: "ORPHANED_OBJECT"
          key: "ORPHANED_OBJECT"
"#;
        let projected = spc_projected_keys(drifted);
        assert!(projected.contains("SESSION_SECRET"));
        // ORPHANED_OBJECT is declared in secretObjects but has no source
        // path. The name is deliberately fictional: this fixture is the drift
        // shape, and naming a real key here would read as a claim that some
        // deployment projects it.
        assert!(!projected.contains("ORPHANED_OBJECT"));
    }

    /// Every key the `SecretProviderClass` projects. This is the mount
    /// contract in full: the CSI driver creates a Secret Manager object per
    /// entry, mounts it, and `envFrom` hands the whole set to `web` and
    /// `workflows-service` as environment.
    ///
    /// Stated as a closed set rather than a floor. A key reaches a pod only
    /// by appearing here, so enumerating the list makes the class's own
    /// contents reviewable in one place: adding a key is a visible edit to
    /// this constant, and a key that no longer belongs leaves it. The
    /// companion floor is [`secret_provider_class_declares_every_boot_required_secret_key`],
    /// which reads the same keys against `WEB_REQUIREMENTS`.
    const MOUNT_CONTRACT: &[&str] = &[
        "DOCUSIGN_ACCOUNT_ID",
        "DOCUSIGN_BASE_URL",
        "DOCUSIGN_HMAC_KEY",
        "DOCUSIGN_INTEGRATION_KEY",
        "DOCUSIGN_OAUTH_BASE",
        "DOCUSIGN_PRIVATE_KEY",
        "DOCUSIGN_SIGNER_EMAIL",
        "DOCUSIGN_USER_ID",
        "DOCUSIGN_WEBHOOK_SECRET",
        "NAVIGATOR_CREDENTIAL_ENVIRONMENT",
        "NAVIGATOR_ENVIRONMENT",
        "NAVIGATOR_FORGE_BACKEND",
        "NAVIGATOR_GITHUB_APP_ID",
        "NAVIGATOR_GITHUB_APP_LOGIN",
        "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
        "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
        "NAVIGATOR_GITHUB_INSTALLATION_ID",
        "NAVIGATOR_GITHUB_ORG",
        "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
        "NAVIGATOR_GIT_WRITER_TOKEN",
        "NAVIGATOR_SURREAL_ARCHIVES_BUCKET",
        "NAVIGATOR_SURREAL_DATABASE",
        "NAVIGATOR_SURREAL_ENDPOINT",
        "NAVIGATOR_SURREAL_NAMESPACE",
        "NAVIGATOR_SURREAL_PASSWORD",
        "NAVIGATOR_SURREAL_USER",
        "OAUTH_CLIENT_SECRET",
        "OAUTH_MICROSOFT_CLIENT_SECRET",
        "RESTATE_AUTH_TOKEN",
        "RESTATE_BROKER_URL",
        "RESTATE_INGRESS_URL",
        "SENDGRID_API_KEY",
        "SENDGRID_EVENTS_PUBLIC_KEY",
        "SENDGRID_EVENTS_SECRET",
        "SENDGRID_FROM_EMAIL",
        "SENDGRID_INBOUND_SECRET",
        "SESSION_SECRET",
    ];

    /// The class projects the mount contract exactly.
    ///
    /// The store reaches a pod through the six `NAVIGATOR_SURREAL_*` keys
    /// listed here, and a credential for any other engine would have to join
    /// them to be mounted at all — which this equality reports as a
    /// difference rather than letting it ride along unnoticed.
    #[test]
    fn the_secret_provider_class_projects_exactly_the_mount_contract() {
        let expected: BTreeSet<String> = MOUNT_CONTRACT.iter().map(ToString::to_string).collect();
        let projected = secret_provider_class_keys();

        let unexpected: Vec<&String> = projected.difference(&expected).collect();
        let absent: Vec<&String> = expected.difference(&projected).collect();
        assert!(
            unexpected.is_empty() && absent.is_empty(),
            "the SecretProviderClass no longer projects the mount contract. Projected but not \
             listed: {unexpected:?}. Listed but not projected: {absent:?}. Every key here is \
             created in Secret Manager and handed to `web` as environment, so reconcile \
             examples/deploy/k8s/gke/secrets/secret-provider-class.yaml and MOUNT_CONTRACT \
             together.",
        );
    }

    /// Every boot-required Secret key `web` enforces at startup must be
    /// declared in the `SecretProviderClass`. Otherwise activating the
    /// Secret Manager CSI sync would project a `navigator-web-secrets`
    /// missing that key and crash-loop the pod — the #591 regression
    /// (a new `WEB_REQUIREMENTS` key added with no provisioning) this guards.
    #[test]
    fn secret_provider_class_declares_every_boot_required_secret_key() {
        let satisfied: BTreeSet<String> = secret_provider_class_keys()
            .into_iter()
            .chain(INLINE_ENV_WEB_KEYS.iter().map(ToString::to_string))
            .collect();
        let required = shared_web_requirements(store::deployment::GITHUB_AUTOMATION_HOME_PROJECT);
        let effective = effective_requirements(&required, &satisfied);
        let missing = unsatisfied_requirements(&effective, &satisfied);
        assert!(
            missing.is_empty(),
            "SecretProviderClass secretObjects is missing boot-required keys: {}. \
             Declare them in examples/deploy/k8s/gke/secrets/secret-provider-class.yaml.",
            missing
                .iter()
                .map(SecretRequirement::describe)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// A rendered stream carrying one `SecretProviderClass` that references
    /// two objects, plus an unrelated document the scan must ignore.
    const RENDERED_SPC_STREAM: &str = r#"
apiVersion: v1
kind: Namespace
metadata:
  name: navigator
---
apiVersion: secrets-store.csi.x-k8s.io/v1
kind: SecretProviderClass
metadata:
  name: navigator-web
spec:
  provider: gke
  parameters:
    secrets: |
      - resourceName: "projects/neon-law-stg/secrets/SESSION_SECRET/versions/latest"
        path: "SESSION_SECRET"
      - resourceName: "projects/neon-law-stg/secrets/RESTATE_AUTH_TOKEN/versions/latest"
        path: "RESTATE_AUTH_TOKEN"
  secretObjects:
    - secretName: neon-law-stg-web-secrets
      data:
        - objectName: "SESSION_SECRET"
          key: "SESSION_SECRET"
"#;

    fn object(secret_id: &str) -> ProjectedObject {
        ProjectedObject {
            project_id: "neon-law-stg".into(),
            secret_id: secret_id.into(),
            version: "latest".into(),
        }
    }

    #[test]
    fn referenced_objects_come_from_the_built_stream_with_their_full_coordinate() {
        let referenced = referenced_secret_manager_objects(RENDERED_SPC_STREAM)
            .expect("the rendered stream parses");
        assert_eq!(
            referenced.into_iter().collect::<Vec<_>>(),
            vec![object("RESTATE_AUTH_TOKEN"), object("SESSION_SECRET")]
        );
    }

    #[test]
    fn a_stream_with_no_secret_provider_class_references_nothing() {
        // Before the CSI resource is wired into the kustomization the class is
        // never applied, so there is nothing to resolve and the preflight must
        // not invent work — or a pre-activation ship would demand objects the
        // deployment has no reason to hold yet.
        let referenced = referenced_secret_manager_objects(RENDERED_MANIFESTS)
            .expect("the rendered stream parses");
        assert!(referenced.is_empty());
    }

    #[test]
    fn a_malformed_resource_name_aborts_rather_than_checking_nothing() {
        // A half-substituted or hand-edited reference would resolve to no
        // object at all. Skipping it silently is the one outcome that turns
        // this preflight into decoration.
        let broken = RENDERED_SPC_STREAM.replace(
            "projects/neon-law-stg/secrets/RESTATE_AUTH_TOKEN/versions/latest",
            "secrets/RESTATE_AUTH_TOKEN",
        );
        let error = referenced_secret_manager_objects(&broken)
            .expect_err("a malformed resourceName must abort");
        assert!(
            error.to_string().contains("secrets/RESTATE_AUTH_TOKEN"),
            "{error}"
        );
    }

    #[test]
    fn every_referenced_object_resolving_lets_the_ship_proceed() {
        let states = vec![
            (object("SESSION_SECRET"), Some("ENABLED".to_string())),
            (object("RESTATE_AUTH_TOKEN"), Some("ENABLED".to_string())),
        ];
        check_projected_objects_resolve(&states)
            .expect("a project holding every referenced object must let the ship proceed");
    }

    #[test]
    fn a_missing_object_aborts_the_ship_naming_it_and_the_count() {
        // The ENG-62 blocker as a unit test: the manifest referenced 29
        // objects while the project held 28, and the gap only surfaced as a
        // crash-looping pod. Both counts belong in the message, because the
        // mismatch is the finding.
        let states = vec![
            (object("SESSION_SECRET"), Some("ENABLED".to_string())),
            (object("DOCUSIGN_ACCESS_TOKEN"), None),
        ];
        let error = check_projected_objects_resolve(&states)
            .expect_err("a missing object must abort the ship");
        let message = error.to_string();
        assert!(message.contains("DOCUSIGN_ACCESS_TOKEN"), "{message}");
        assert!(message.contains("no such object"), "{message}");
        assert!(message.contains("references 2"), "{message}");
        assert!(message.contains("only 1"), "{message}");
        assert!(message.contains("neon-law-stg"), "{message}");
        assert!(message.contains("Nothing was applied"), "{message}");
    }

    #[test]
    fn a_disabled_version_aborts_as_firmly_as_a_missing_one() {
        // A disabled version answers the metadata read, so a check that only
        // looked for existence would pass while the driver still cannot serve
        // the mount.
        let states = vec![(object("SESSION_SECRET"), Some("DISABLED".to_string()))];
        let error = check_projected_objects_resolve(&states)
            .expect_err("a disabled version cannot be mounted");
        assert!(error.to_string().contains("SESSION_SECRET (DISABLED)"));
    }

    #[test]
    fn the_built_overlay_mounts_the_csi_volume_and_references_only_projected_objects() {
        // The CSI chain is active, and "active" is a property of the BUILT
        // stream, not of the two files under `secrets/`. A `resources:` entry
        // that never resolves or a patch whose target does not match would
        // leave both manifests correct and the deployment reading a plain
        // Secret nobody reconciles any more.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.19.20",
            env_getter(FULL_ENV),
        )
        .expect("full environment resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let manifests = kustomize_build(&rendered.path().join(GKE_KUSTOMIZE_SUBPATH))
            .expect("rendered GKE manifests build");

        let referenced =
            referenced_secret_manager_objects(&manifests).expect("the built stream parses");
        assert!(
            !referenced.is_empty(),
            "the SecretProviderClass must be in the built overlay"
        );
        for object in &referenced {
            assert_eq!(object.project_id, "neon-law-420305");
            assert_eq!(object.version, "latest");
        }

        // Every referenced object is projected and every projected key is
        // referenced. The two blocks drifting apart is how an object becomes
        // unmountable (referenced, never written) or a key silently absent
        // (projected, never sourced).
        let referenced_ids: BTreeSet<String> = referenced
            .iter()
            .map(|object| object.secret_id.clone())
            .collect();
        assert_eq!(referenced_ids, secret_provider_class_keys());

        // …and `web` actually mounts the volume, which is the only reason the
        // driver reconciles the projected Secret at all.
        let web = manifest_doc(&manifests, "Deployment", "navigator-web");
        let volumes = web
            .get("spec")
            .and_then(|spec| spec.get("template"))
            .and_then(|template| template.get("spec"))
            .and_then(|spec| spec.get("volumes"))
            .and_then(serde_yaml::Value::as_sequence)
            .expect("the web Deployment declares volumes");
        assert!(
            volumes.iter().any(|volume| {
                volume
                    .get("csi")
                    .and_then(|csi| csi.get("volumeAttributes"))
                    .and_then(|attributes| attributes.get("secretProviderClass"))
                    .and_then(serde_yaml::Value::as_str)
                    == Some("navigator-web")
            }),
            "the web pod must mount the CSI volume: {volumes:?}"
        );
    }

    #[test]
    fn check_secret_invariants_passes_when_the_secret_satisfies_every_requirement() {
        // The abort-or-proceed decision itself, against a manifest stream —
        // no cluster. This is the `==> Secret invariants OK` path that lets
        // a ship reach the apply.
        check_secret_invariants(
            &sample_config(),
            RENDERED_MANIFESTS,
            &satisfying_secret_keys(),
        )
        .expect("a Secret carrying every requirement must let the ship proceed");
    }

    #[test]
    fn check_secret_invariants_aborts_naming_the_missing_keys() {
        // The 26.7.15 failure, reproduced as a unit test: a Secret missing
        // the GitHub App trio must abort the ship BEFORE the reconcile, and
        // say which keys and which deployments.
        let mut keys = satisfying_secret_keys();
        for absent in [
            "NAVIGATOR_GITHUB_ORG",
            "NAVIGATOR_GITHUB_APP_ID",
            "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
        ] {
            keys.remove(absent);
        }
        let err = check_secret_invariants(&sample_config(), RENDERED_MANIFESTS, &keys)
            .expect_err("a Secret missing a boot requirement must abort the ship");
        let message = err.to_string();
        assert!(message.contains("navigator-web"), "{message}");
        assert!(
            message.contains("NAVIGATOR_GITHUB_APP_PRIVATE_KEY"),
            "{message}"
        );
        // And the remedy is one paste-able patch, not one key.
        assert!(
            message.contains(r#""NAVIGATOR_GITHUB_ORG":"<value>""#),
            "{message}"
        );
    }

    #[test]
    fn check_secret_invariants_fails_closed_on_unparseable_manifests() {
        // If the manifest stream can't be parsed we must NOT sail past the
        // preflight having checked nothing — finding zero deployments would
        // otherwise read as "no missing keys" and wave a bad ship through.
        let err = check_secret_invariants(
            &sample_config(),
            "kind: Deployment\n  bad: [indent",
            &satisfying_secret_keys(),
        )
        .expect_err("unparseable manifests must abort the ship, not pass it");
        assert!(
            err.to_string().contains("rendered manifest stream"),
            "{err}"
        );
    }

    #[test]
    fn check_secret_invariants_ignores_a_requirement_whose_trigger_is_absent() {
        // `OIDC_AUDIENCE` is required only when `OIDC_JWKS_URL` is
        // configured. With no JWKS url anywhere, demanding the audience
        // would be a false positive that stops a ship for nothing — the
        // pod's own invariant skips it too.
        let mut keys = satisfying_secret_keys();
        keys.remove("OIDC_JWKS_URL");
        keys.remove("OIDC_AUDIENCE");
        keys.remove("OIDC_ISSUER");
        check_secret_invariants(&sample_config(), RENDERED_MANIFESTS, &keys)
            .expect("an untriggered conditional requirement must not block the ship");
    }

    #[test]
    fn rendered_gke_web_deployment_supplies_the_clamd_address() {
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.19.20",
            env_getter(FULL_ENV),
        )
        .expect("full environment resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let manifests = kustomize_build(&rendered.path().join(GKE_KUSTOMIZE_SUBPATH))
            .expect("rendered GKE manifests build");
        let deployment_envs =
            parse_web_binary_envs(&manifests, "neon-production-web-secrets", &BTreeSet::new())
                .expect("rendered manifest stream parses");
        let missing = missing_requirements_by_deployment(
            &[req(&[&["NAVIGATOR_CLAMD_ADDR"]])],
            &BTreeSet::new(),
            &deployment_envs,
        );

        assert!(
            missing.is_empty(),
            "every web-binary deployment needs the ClamAV address: {missing:?}"
        );
    }

    #[test]
    fn private_mode_puts_the_basic_auth_gateway_in_front_of_web() {
        // The whole point of the flag: with it on, the Service must stop
        // targeting the app port and start targeting nginx, and the pod
        // must carry the gateway. Assert on the BUILT stream, not on the
        // component files, because the component only works if kustomize
        // actually merges it into the GKE tree — a wrong `components:`
        // path or a patch that misses its target would leave the files
        // correct and the deployment public.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.19.20",
            env_getter(FULL_ENV),
        )
        .expect("full environment resolves");
        let rendered = render_manifests_with(&subs, true).expect("render succeeds");
        let manifests = kustomize_build(&rendered.path().join(GKE_KUSTOMIZE_SUBPATH))
            .expect("rendered GKE manifests build with the private-mode component");

        let service = manifest_doc(&manifests, "Service", "navigator-web");
        let target_port = service["spec"]["ports"][0]["targetPort"].as_u64();
        assert_eq!(
            target_port,
            Some(8080),
            "the Service must reach `web` through the gateway, not directly: {:?}",
            service["spec"]["ports"]
        );

        let deployment = manifest_doc(&manifests, "Deployment", "navigator-web");
        let containers = deployment["spec"]["template"]["spec"]["containers"]
            .as_sequence()
            .expect("the web pod has containers");
        let gateway = containers
            .iter()
            .find(|c| c["name"].as_str() == Some("private-gateway"))
            .expect("private mode adds the Pingora sidecar");
        assert_eq!(
            gateway["readinessProbe"]["httpGet"]["path"].as_str(),
            Some("/health"),
            "the probe must target the one unauthenticated location, or the LB health check 401s"
        );
        assert_eq!(
            gateway["ports"][0]["containerPort"].as_u64(),
            target_port,
            "the gateway must listen on the port the Service targets. A numeric `targetPort` is \
             published for every selected pod whether or not anything there listens, so a \
             disagreement here is not a routing miss that fails closed — it is a live upstream \
             where nothing answers, and the load balancer turns it into a 502."
        );
        assert_eq!(
            gateway["image"].as_str(),
            Some("ghcr.io/neon-law-source-code/navigator-gateway:26.7.19.20")
        );
        assert!(
            manifests.contains("navigator-private-basic-auth"),
            "the basic-auth Secret must be part of the applied tree"
        );
    }

    #[test]
    fn the_default_ship_stays_public() {
        // The other half of the toggle, and the one that matters more: a
        // ship with private mode off must render the tree it always did.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.19.20",
            env_getter(FULL_ENV),
        )
        .expect("full environment resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let manifests = kustomize_build(&rendered.path().join(GKE_KUSTOMIZE_SUBPATH))
            .expect("rendered GKE manifests build");

        assert!(!manifests.contains("private-gateway"), "{manifests}");
        let service = manifest_doc(&manifests, "Service", "navigator-web");
        assert_eq!(
            service["spec"]["ports"][0]["targetPort"].as_u64(),
            Some(3001)
        );
    }

    #[test]
    fn enabling_private_mode_twice_is_refused() {
        // `enable_private_mode` appends a top-level key. If the GKE
        // kustomization ever grows its own `components:`, appending a
        // second one yields a duplicate mapping key that kustomize
        // rejects — mid-ship, after the preflight has passed. Fail here
        // instead, naming the fix.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.19.20",
            env_getter(FULL_ENV),
        )
        .expect("full environment resolves");
        let once =
            enable_private_mode("resources:\n  - clamav.yaml\n", &subs).expect("first append");
        assert!(once.contains(PRIVATE_MODE_COMPONENT));
        let err = enable_private_mode(&once, &subs).expect_err("a second append must abort");
        assert!(err.to_string().contains("already declares"), "{err}");
    }

    #[test]
    fn render_pins_a_same_day_h_tag() {
        // `YY.M.D` is the substitution TOKEN, not a shape constraint: a
        // legacy ad-hoc same-day release carries a fourth `.H` group. The render must pin it
        // verbatim rather than assume three components.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15.4",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let web_image = fs::read_to_string(
            rendered
                .path()
                .join(GKE_KUSTOMIZE_SUBPATH)
                .join("patches/web-image.yaml"),
        )
        .unwrap();
        assert!(web_image.contains("neon-server:26.7.15.4"), "{web_image}");
        assert!(!web_image.contains(RELEASE_TAG_TOKEN));
    }

    #[test]
    fn render_pins_a_hotfix_tag_verbatim() {
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.8.19-hotfix.14",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let web_image = fs::read_to_string(
            rendered
                .path()
                .join(GKE_KUSTOMIZE_SUBPATH)
                .join("patches/web-image.yaml"),
        )
        .unwrap();
        assert!(
            web_image.contains("neon-server:26.8.19-hotfix.14"),
            "{web_image}"
        );
        assert!(!web_image.contains(RELEASE_TAG_TOKEN));
    }

    #[test]
    fn substitutions_do_not_corrupt_one_another() {
        // `apply_substitutions` is a fold of plain string replacements, so
        // its correctness rests on the tokens being non-overlapping and no
        // substituted VALUE containing another token. Guard that here
        // rather than in the comment that currently asserts it.
        let subs = resolve_substitutions_for_deployment(
            "neon-production",
            "26.7.15",
            env_getter(FULL_ENV),
        )
        .expect("full env resolves");
        for sub in &subs {
            for other in &subs {
                assert!(
                    !sub.value.contains(other.token),
                    "value of `{}` contains token `{}` — the fold order would matter",
                    sub.token,
                    other.token
                );
            }
        }
        let tokens: Vec<&str> = subs.iter().map(|s| s.token).collect();
        for (i, token) in tokens.iter().enumerate() {
            for (j, other) in tokens.iter().enumerate() {
                assert!(
                    i == j || !token.contains(other),
                    "token `{token}` contains `{other}` — replacement order would matter"
                );
            }
        }
    }

    #[test]
    fn suggested_patch_keys_degrade_to_a_valid_command() {
        // A requirement with no alternatives names no key. The command
        // shape must stay valid rather than emit an empty patch object.
        let by_deployment = vec![("navigator-web".to_string(), vec![req(&[])])];
        assert_eq!(suggested_patch_keys(&by_deployment), vec!["KEY"]);
        assert_eq!(
            secret_patch_stringdata(&suggested_patch_keys(&by_deployment)),
            r#"{"stringData":{"KEY":"<value>"}}"#
        );
    }

    #[test]
    fn rendered_envs_reject_unparseable_manifests() {
        // `kubectl kustomize` emitting something we can't parse must fail
        // the ship, not silently yield zero deployments and "pass" the
        // preflight by finding nothing to check.
        let err = parse_web_binary_envs("kind: Deployment\n  bad: [indent", "s", &BTreeSet::new())
            .expect_err("unparseable manifests must not pass the preflight");
        assert!(
            err.to_string().contains("rendered manifest stream"),
            "{err}"
        );
    }

    #[test]
    fn rendered_envs_skip_a_deployment_with_no_name() {
        // A nameless Deployment can't be matched to the web tier; skip it
        // rather than panic, and still fail closed on finding no web tier.
        let err = parse_web_binary_envs(
            "apiVersion: apps/v1\nkind: Deployment\nspec: {}\n",
            "s",
            &BTreeSet::new(),
        )
        .expect_err("a nameless Deployment is not a web tier");
        assert!(err.to_string().contains("cannot be boot-checked"), "{err}");
    }

    #[test]
    fn rendered_envs_reject_a_tree_with_no_web_tier() {
        // A tree that renders no web-binary Deployment cannot be
        // boot-checked. Failing closed beats shipping an unchecked web tier.
        let err = parse_web_binary_envs(
            "apiVersion: v1\nkind: Service\nmetadata:\n  name: navigator-web\n",
            "navigator-web-secrets",
            &BTreeSet::new(),
        )
        .expect_err("a tree with no web-binary Deployment must not pass the preflight");
        assert!(err.to_string().contains("cannot be boot-checked"), "{err}");
    }

    #[test]
    fn suggested_patch_keys_cover_every_missing_requirement_once() {
        // The real prod shape: the web deployment misses three
        // requirements out of the one shared Secret. The operator must get
        // every key in a single patch — a one-key example sends them round
        // the ship loop once per key.
        let missing = vec![
            req(&[&["SENDGRID_FROM_EMAIL"]]),
            req(&[&[
                "NAVIGATOR_GITHUB_ORG",
                "NAVIGATOR_GITHUB_APP_ID",
                "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
            ]]),
            req(&[&["NAVIGATOR_CREDENTIAL_ENVIRONMENT"]]),
        ];
        let by_deployment = vec![("navigator-web".to_string(), missing)];
        assert_eq!(
            suggested_patch_keys(&by_deployment),
            vec![
                "SENDGRID_FROM_EMAIL",
                "NAVIGATOR_GITHUB_ORG",
                "NAVIGATOR_GITHUB_APP_ID",
                "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
                "NAVIGATOR_CREDENTIAL_ENVIRONMENT",
            ]
        );
    }

    #[test]
    fn suggested_patch_keys_take_the_first_alternative() {
        // `A or B + C` — suggest the single-key `A`, not a mix of both
        // alternatives, which would over-ask for keys the deployer may
        // not hold.
        let by_deployment = vec![(
            "navigator-web".to_string(),
            vec![req(&[&["DOCUSIGN_ACCESS_TOKEN"], &["DOCUSIGN_USER_ID"]])],
        )];
        assert_eq!(
            suggested_patch_keys(&by_deployment),
            vec!["DOCUSIGN_ACCESS_TOKEN"]
        );
    }

    #[test]
    fn unsatisfied_secret_error_names_every_missing_key_in_one_patch() {
        // Reproduces the real prod ship failure: the web deployment misses
        // three requirements. The operator must be able to copy ONE command
        // and satisfy all of them.
        let missing = vec![
            req(&[&["SENDGRID_FROM_EMAIL"]]),
            req(&[&[
                "NAVIGATOR_GITHUB_ORG",
                "NAVIGATOR_GITHUB_APP_ID",
                "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
            ]]),
            req(&[&["NAVIGATOR_CREDENTIAL_ENVIRONMENT"]]),
        ];
        let message =
            unsatisfied_secret_error(&sample_config(), &[("navigator-web".to_string(), missing)]);
        // The diagnosis still names the deployment and the `A + B`
        // phrasing the invariant uses.
        assert!(message.contains("navigator-web: SENDGRID_FROM_EMAIL"));
        assert!(message.contains(
            "NAVIGATOR_GITHUB_ORG + NAVIGATOR_GITHUB_APP_ID + NAVIGATOR_GITHUB_APP_PRIVATE_KEY"
        ));
        // The remedy carries all five keys, not just the first.
        assert!(message.contains(
            r#"-p '{"stringData":{"SENDGRID_FROM_EMAIL":"<value>","NAVIGATOR_GITHUB_ORG":"<value>","NAVIGATOR_GITHUB_APP_ID":"<value>","NAVIGATOR_GITHUB_APP_PRIVATE_KEY":"<value>","NAVIGATOR_CREDENTIAL_ENVIRONMENT":"<value>"}}'"#
        ), "patch must carry every missing key; got:\n{message}");
        assert!(message.contains(
            "kubectl --context gke_my-org-prod_us-west4_navigator -n navigator \
             patch secret navigator-web-secrets --type=merge"
        ));
    }

    #[test]
    fn secret_patch_stringdata_is_a_valid_merge_patch() {
        let body = secret_patch_stringdata(&["A".to_string(), "B".to_string()]);
        assert_eq!(body, r#"{"stringData":{"A":"<value>","B":"<value>"}}"#);
        // Paste-ability is the whole point: it has to parse as the JSON
        // `kubectl patch --type=merge` will read.
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(parsed["stringData"]["B"], "<value>");
    }

    #[test]
    fn empty_secret_values_do_not_satisfy() {
        // An empty-valued key crash-loops the pod exactly like a missing
        // one (`enforce_deployment_invariants` treats empty as unset), so it
        // must not enter the satisfied set: the integration key present and
        // the private key empty must leave the DocuSign JWT requirement
        // unsatisfied.
        let secret = serde_json::json!({
            "data": {
                "DOCUSIGN_INTEGRATION_KEY": "aW50ZWdyYXRpb24ta2V5",
                "DOCUSIGN_PRIVATE_KEY": "",
            }
        });
        let satisfied = populated_secret_keys(&secret);
        assert!(satisfied.contains("DOCUSIGN_INTEGRATION_KEY"));
        assert!(!satisfied.contains("DOCUSIGN_PRIVATE_KEY"));

        let jwt_grant = req(&[
            &["DOCUSIGN_ACCESS_TOKEN"],
            &["DOCUSIGN_INTEGRATION_KEY", "DOCUSIGN_PRIVATE_KEY"],
        ]);
        assert!(!jwt_grant.is_satisfied_by(&satisfied));
    }

    #[test]
    fn env_names_count_only_usable_declarations() {
        // A non-empty literal and a valueFrom reference the preflight
        // can't inspect (a different Secret "s") satisfy; a name-only or
        // empty-literal declaration resolves to an empty string in the
        // pod, which the boot invariant treats as unset.
        let deployment = serde_json::json!({
            "spec": { "template": { "spec": { "containers": [{
                "env": [
                    { "name": "EMPTY_LITERAL", "value": "" },
                    { "name": "NAME_ONLY" },
                    { "name": "FROM_SECRET",
                      "valueFrom": { "secretKeyRef": { "name": "s", "key": "k" } } },
                ]
            }] } } }
        });
        // The shipped Secret is a different one, so the secretKeyRef to
        // "s" stays optimistic and counts.
        let names = populated_env_names(&deployment, "navigator-web-secrets", &BTreeSet::new());
        let expected: BTreeSet<String> = ["FROM_SECRET"].into_iter().map(String::from).collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn secret_ref_into_the_shipped_secret_resolves_against_its_values() {
        // A `secretKeyRef` into the very Secret `ship` inspects is
        // resolvable, so an empty referenced value must NOT count as
        // usable — otherwise this env path re-adds a key that
        // `populated_secret_keys` correctly dropped, passing the
        // preflight while the pod crash-loops. A ref to a *different*
        // Secret can't be inspected and stays optimistic.
        let deployment = serde_json::json!({
            "spec": { "template": { "spec": { "containers": [{
                "env": [
                    // Populated key in the shipped Secret → usable.
                    { "name": "DOCUSIGN_INTEGRATION_KEY",
                      "valueFrom": { "secretKeyRef": {
                          "name": "navigator-web-secrets", "key": "DOCUSIGN_INTEGRATION_KEY" } } },
                    // Empty key in the shipped Secret → NOT usable.
                    { "name": "DOCUSIGN_PRIVATE_KEY",
                      "valueFrom": { "secretKeyRef": {
                          "name": "navigator-web-secrets", "key": "DOCUSIGN_PRIVATE_KEY" } } },
                    // Ref into a different Secret → uninspectable, optimistic.
                    { "name": "FROM_OTHER",
                      "valueFrom": { "secretKeyRef": { "name": "other", "key": "k" } } },
                ]
            }] } } }
        });
        // The shipped Secret has the integration key populated but the
        // private key empty (only the populated key is in the set
        // `populated_secret_keys` returns).
        let secret_keys: BTreeSet<String> = ["DOCUSIGN_INTEGRATION_KEY"]
            .into_iter()
            .map(String::from)
            .collect();
        let names = populated_env_names(&deployment, "navigator-web-secrets", &secret_keys);
        let expected: BTreeSet<String> = ["DOCUSIGN_INTEGRATION_KEY", "FROM_OTHER"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            names, expected,
            "empty secretKeyRef into the shipped Secret must not be counted usable"
        );

        // End-to-end: the JWT requirement stays unsatisfied when the
        // private key is wired via an empty secretKeyRef into the shipped
        // Secret — a preflight-passing crash-loop otherwise.
        let jwt_grant = req(&[
            &["DOCUSIGN_ACCESS_TOKEN"],
            &["DOCUSIGN_INTEGRATION_KEY", "DOCUSIGN_PRIVATE_KEY"],
        ]);
        let satisfied: BTreeSet<String> = secret_keys.union(&names).cloned().collect();
        assert!(!jwt_grant.is_satisfied_by(&satisfied));
    }

    #[test]
    fn web_signing_iam_binding_grants_token_creator_on_the_web_gsa_itself() {
        // Regression guard for the document-download 500. The web pod signs
        // GCS URLs via IAM `signBlob`, which needs the web GSA to be a
        // serviceAccountTokenCreator on ITSELF — the binding must target the
        // GSA's own resource and name the same GSA as the member.
        let gsa = "navigator-web@my-org-prod.iam.gserviceaccount.com";
        let args = web_signing_iam_binding_args("my-org-prod", "navigator-web");

        assert_eq!(
            args[..3],
            ["iam", "service-accounts", "add-iam-policy-binding"],
            "must bind on the service-account resource, not the project policy"
        );
        assert!(
            args.contains(&gsa.to_string()),
            "the binding must target the web GSA's own resource: {args:?}"
        );
        assert!(
            args.contains(&"roles/iam.serviceAccountTokenCreator".to_string()),
            "signBlob requires the token-creator role: {args:?}"
        );
        assert!(
            args.contains(&format!("--member=serviceAccount:{gsa}")),
            "the GSA must be a token-creator on itself: {args:?}"
        );
        assert!(
            args.contains(&"--project=my-org-prod".to_string()),
            "the binding must be scoped to the deploy project: {args:?}"
        );
    }

    #[test]
    fn web_gsa_email_is_the_kind_bound_workload_identity_principal() {
        assert_eq!(
            web_gsa_email("my-org-prod", "navigator-web"),
            "navigator-web@my-org-prod.iam.gserviceaccount.com"
        );
    }

    #[test]
    fn dry_run_still_reads_the_policy() {
        // The regression this step exists to prevent: a dry-run that skipped
        // the read reported `ship complete` for a roll that could not clear
        // step 1c. The read is not conditioned on the mode, so a dry-run must
        // call it — and must fail when it is denied, exactly as a live roll
        // would, rather than printing a line and returning Ok.
        let mut read_calls = 0;
        let bound = policy_with(
            "roles/iam.serviceAccountTokenCreator",
            &format!(
                "serviceAccount:{}",
                web_gsa_email("my-org-prod", "navigator-web")
            ),
        );
        ensure_web_signing_iam_with(&sample_config(), true, SigningIamAuthority::Write, || {
            read_calls += 1;
            Ok(bound)
        })
        .expect("a bound policy clears the step");
        assert_eq!(read_calls, 1, "the dry-run must perform the read");
    }

    #[test]
    fn dry_run_fails_when_the_policy_cannot_be_read() {
        // The other half of the same property: a denied read is the answer to
        // "would the real roll work", so it must sink the dry-run too.
        let err =
            ensure_web_signing_iam_with(&sample_config(), true, SigningIamAuthority::Write, || {
                Err(signing_iam_read_failed(
                    "gsa@example.com",
                    "PERMISSION_DENIED",
                ))
            })
            .expect_err("a denied read must fail the dry-run");

        assert!(err.to_string().contains("cannot verify"), "{err}");
    }

    #[test]
    fn dry_run_prints_the_write_instead_of_performing_it() {
        // The write is the only half a dry-run declines: with the binding
        // absent it reports what the live roll would do and returns Ok rather
        // than shelling out to gcloud.
        let empty = serde_json::json!({ "etag": "BwYb0000000=" });
        ensure_web_signing_iam_with(&sample_config(), true, SigningIamAuthority::Write, || {
            Ok(empty)
        })
        .expect("dry-run must not attempt the IAM write");
    }

    /// A `get-iam-policy` document in the shape gcloud returns, granting
    /// `role` to `member`.
    fn policy_with(role: &str, member: &str) -> serde_json::Value {
        serde_json::json!({
            "bindings": [
                { "role": "roles/iam.workloadIdentityUser",
                  "members": ["serviceAccount:neon-law-stg.svc.id.goog[navigator/navigator-web]"] },
                { "role": role, "members": [member] },
            ],
            "etag": "BwYb0000000=",
            "version": 1,
        })
    }

    #[test]
    fn web_signing_iam_read_needs_only_get_iam_policy() {
        // The steady-state call. It must be a READ on the GSA's own resource:
        // that is what drops the operator credential from setIamPolicy to
        // getIamPolicy on a row that is already bound.
        let args = web_signing_iam_read_args("neon-law-stg", "neon-law-stg-web");

        assert_eq!(
            args[..3],
            ["iam", "service-accounts", "get-iam-policy"],
            "the verify half must read, not write: {args:?}"
        );
        assert!(
            args.contains(&"neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com".to_string()),
            "the read must target the web GSA's own resource: {args:?}"
        );
        assert!(
            args.contains(&"--project=neon-law-stg".to_string()),
            "the read must be scoped to the deploy project: {args:?}"
        );
        assert!(
            args.contains(&"--format=json".to_string()),
            "the verdict is decided by parsing the policy, so it must be JSON: {args:?}"
        );
    }

    #[test]
    fn a_present_self_binding_needs_no_write() {
        // The steady state on a provisioned row: `ops gcp setup` already wrote
        // this binding, so the roll must recognise it and skip the setIamPolicy.
        let gsa = "neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com";
        let policy = policy_with(
            "roles/iam.serviceAccountTokenCreator",
            &format!("serviceAccount:{gsa}"),
        );

        assert!(
            policy_grants_self_signing(&policy, gsa),
            "an unconditional self-binding is the no-write case"
        );
    }

    #[test]
    fn a_missing_self_binding_is_written() {
        // Three ways the grant can be absent, each of which must fall through
        // to the binding write rather than pass the verify.
        let gsa = "neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com";

        let empty = serde_json::json!({ "etag": "BwYb0000000=" });
        assert!(
            !policy_grants_self_signing(&empty, gsa),
            "a policy with no bindings at all grants nothing"
        );

        let other_member = policy_with(
            "roles/iam.serviceAccountTokenCreator",
            "serviceAccount:someone-else@neon-law-stg.iam.gserviceaccount.com",
        );
        assert!(
            !policy_grants_self_signing(&other_member, gsa),
            "token-creator held by another principal does not let the pod sign for itself"
        );

        let other_role = policy_with(
            "roles/iam.serviceAccountUser",
            &format!("serviceAccount:{gsa}"),
        );
        assert!(
            !policy_grants_self_signing(&other_role, gsa),
            "only serviceAccountTokenCreator carries signBlob"
        );
    }

    #[test]
    fn a_conditional_self_binding_does_not_satisfy_the_invariant() {
        // The pod signs on every download, at an hour no condition here can be
        // read to cover, so a conditional grant is treated as absent.
        let gsa = "neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com";
        let policy = serde_json::json!({
            "bindings": [{
                "role": "roles/iam.serviceAccountTokenCreator",
                "members": [format!("serviceAccount:{gsa}")],
                "condition": {
                    "title": "expires",
                    "expression": "request.time < timestamp(\"2030-01-01T00:00:00Z\")",
                },
            }],
        });

        assert!(
            !policy_grants_self_signing(&policy, gsa),
            "a conditional grant is not the unconditional invariant the roll asserts"
        );
    }

    #[test]
    fn assert_only_refuses_an_absent_binding_instead_of_writing() {
        // ENG-311. The residual this flag exists for: a LIVE roll (dry_run
        // false) whose binding is absent must stop at the verify rather than
        // reach for setIamPolicy. Nothing here is mocked past the read, so a
        // regression that took the write branch would shell out to gcloud —
        // the refusal, and its wording, is what proves it did not.
        let empty = serde_json::json!({ "etag": "BwYb0000000=" });
        let err = ensure_web_signing_iam_with(
            &sample_config(),
            false,
            SigningIamAuthority::AssertOnly,
            || Ok(empty),
        )
        .expect_err("an absent binding must fail the roll under --assert-signing-iam");
        let text = err.to_string();

        assert!(text.contains("--assert-signing-iam"), "{text}");
        assert!(
            text.contains("is missing"),
            "the operator is told the binding is absent, not that it was unreadable: {text}"
        );
        assert!(
            !text.contains("could not be written"),
            "no write was attempted, so this is not the denied-write message: {text}"
        );
    }

    #[test]
    fn assert_only_refuses_under_dry_run_too() {
        // A dry-run answers "would the real roll work". Under this flag the
        // real roll refuses, so a dry-run that returned Ok would be the same
        // false green that moving the read into every mode was meant to end.
        let empty = serde_json::json!({ "etag": "BwYb0000000=" });
        let err = ensure_web_signing_iam_with(
            &sample_config(),
            true,
            SigningIamAuthority::AssertOnly,
            || Ok(empty),
        )
        .expect_err("the dry-run must refuse exactly as the live roll would");

        assert!(err.to_string().contains("--assert-signing-iam"), "{err}");
    }

    #[test]
    fn assert_only_leaves_a_present_binding_untouched() {
        // The flag narrows one branch and must not disturb the steady state:
        // on a provisioned row — every row `ops gcp setup` has touched — the
        // roll still clears the step on the read alone, live and dry alike.
        let bound = policy_with(
            "roles/iam.serviceAccountTokenCreator",
            &format!(
                "serviceAccount:{}",
                web_gsa_email("my-org-prod", "navigator-web")
            ),
        );
        let mut read_calls = 0;

        ensure_web_signing_iam_with(
            &sample_config(),
            false,
            SigningIamAuthority::AssertOnly,
            || {
                read_calls += 1;
                Ok(bound)
            },
        )
        .expect("a bound row clears the step whether or not the roll may write");

        assert_eq!(read_calls, 1, "the verify is still performed: {read_calls}");
    }

    #[test]
    fn the_assert_refusal_names_the_write_permission_and_quotes_the_hand_off() {
        // The operator ran with the flag precisely because they cannot make
        // this grant, so the refusal has to be a hand-off: the permission, a
        // role carrying it, and the verbatim command for whoever holds it.
        let gsa = "neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com";
        let text = signing_iam_assert_failed(
            gsa,
            &web_signing_iam_binding_args("neon-law-stg", "neon-law-stg-web"),
        )
        .to_string();

        assert!(text.contains(gsa), "{text}");
        assert!(text.contains("iam.serviceAccounts.setIamPolicy"), "{text}");
        assert!(text.contains("roles/iam.serviceAccountAdmin"), "{text}");
        assert!(
            text.contains("gcloud iam service-accounts add-iam-policy-binding"),
            "the hand-off must be copy-pasteable: {text}"
        );
        assert!(
            text.contains("roles/iam.serviceAccountTokenCreator"),
            "the quoted command must name the role it grants: {text}"
        );
    }

    #[test]
    fn the_roll_may_write_the_binding_unless_the_flag_withdraws_it() {
        // The wiring. Defaulting this to AssertOnly would silently break every
        // provisioning roll on an unbound row, so the default is asserted here
        // rather than left to `#[derive(Default)]` going unread.
        let mut opts = ShipOpts {
            deployment: "neon-law-stg".into(),
            tag: Some("26.8.22".into()),
            ..ShipOpts::default()
        };
        assert_eq!(
            signing_iam_authority(&opts),
            SigningIamAuthority::Write,
            "an operator who passed no flag keeps the historical behaviour"
        );

        opts.assert_signing_iam = true;
        assert_eq!(
            signing_iam_authority(&opts),
            SigningIamAuthority::AssertOnly,
            "--assert-signing-iam is what withdraws the write"
        );
    }

    #[test]
    fn a_denied_read_names_the_permission_not_a_raw_gcloud_error() {
        // The operator's next move is a grant request, so the message has to
        // carry the permission, a role that holds it, and the resource — and
        // must say we could not VERIFY, not that the binding is missing.
        let gsa = "neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com";
        let text = signing_iam_read_failed(
            gsa,
            "ERROR: (gcloud.iam.service-accounts.get-iam-policy) PERMISSION_DENIED",
        )
        .to_string();

        assert!(text.contains("cannot verify"), "{text}");
        assert!(text.contains("iam.serviceAccounts.getIamPolicy"), "{text}");
        assert!(text.contains("roles/iam.serviceAccountAdmin"), "{text}");
        assert!(text.contains(gsa), "{text}");
        assert!(
            !text.contains("is missing"),
            "an unreadable policy is not a known-absent binding: {text}"
        );
    }

    #[test]
    fn a_denied_write_says_the_binding_is_missing_and_quotes_the_hand_off() {
        // The other half of the distinction: here we know the pod cannot sign.
        let gsa = "neon-law-stg-web@neon-law-stg.iam.gserviceaccount.com";
        let text = signing_iam_write_failed(
            gsa,
            &web_signing_iam_binding_args("neon-law-stg", "neon-law-stg-web"),
            "ERROR: (gcloud.iam.service-accounts.add-iam-policy-binding) PERMISSION_DENIED",
        )
        .to_string();

        assert!(text.contains("is missing"), "{text}");
        assert!(text.contains("iam.serviceAccounts.setIamPolicy"), "{text}");
        assert!(text.contains("roles/iam.serviceAccountAdmin"), "{text}");
        assert!(text.contains("signBlob"), "{text}");
        assert!(
            text.contains(&format!("add-iam-policy-binding {gsa}")),
            "the message must quote the command to hand off: {text}"
        );
    }

    /// A `SecretProviderClass` with the shape the real manifest uses: two
    /// entries per object, one in each block, and comments between them.
    const TWO_OBJECT_SPC: &str = r#"apiVersion: secrets-store.csi.x-k8s.io/v1
kind: SecretProviderClass
metadata:
  name: navigator-web
spec:
  parameters:
    secrets: |
      - resourceName: "projects/p/secrets/RESTATE_AUTH_TOKEN/versions/latest"
        path: "RESTATE_AUTH_TOKEN"
      # A load-bearing comment the filter must not disturb.
      - resourceName: "projects/p/secrets/DOCUSIGN_HMAC_KEY/versions/latest"
        path: "DOCUSIGN_HMAC_KEY"
  secretObjects:
    - secretName: navigator-web-secrets
      data:
        - objectName: "RESTATE_AUTH_TOKEN"
          key: "RESTATE_AUTH_TOKEN"
        - objectName: "DOCUSIGN_HMAC_KEY"
          key: "DOCUSIGN_HMAC_KEY"
"#;

    #[test]
    fn omitting_an_object_removes_it_from_both_blocks() {
        // A `secretObjects` entry left behind after its mount is removed is
        // the drift `spc_projected_keys` was written to catch — the filter must
        // never create it.
        let declined = BTreeSet::from(["DOCUSIGN_HMAC_KEY".to_string()]);
        let filtered =
            without_projected_objects(TWO_OBJECT_SPC, &declined).expect("the filter succeeds");

        assert!(!filtered.contains("DOCUSIGN_HMAC_KEY"));
        assert_eq!(
            projected_object_names(&filtered).expect("the filtered class parses"),
            BTreeSet::from(["RESTATE_AUTH_TOKEN".to_string()])
        );
        assert!(
            filtered.contains("# A load-bearing comment the filter must not disturb."),
            "a line filter is used precisely so the manifest's comments survive"
        );
    }

    #[test]
    fn omitting_nothing_leaves_the_class_byte_identical() {
        // Every deployment that supplies the whole object list — `neon-law-stg`
        // today — must render exactly what it renders now.
        let filtered = without_projected_objects(TWO_OBJECT_SPC, &BTreeSet::new())
            .expect("an empty omission succeeds");
        assert_eq!(filtered, TWO_OBJECT_SPC);
    }

    #[test]
    fn an_entry_that_survives_the_filter_fails_the_render() {
        // The structural backstop. The line filter knows one entry shape; if
        // the manifest ever grows another — here a flow mapping, still valid
        // YAML and still a real reference — the ship must abort rather than
        // reconcile a class whose mount fails on the reference it left behind.
        let unremovable = TWO_OBJECT_SPC.replace(
            "        - objectName: \"DOCUSIGN_HMAC_KEY\"\n          key: \"DOCUSIGN_HMAC_KEY\"\n",
            "        - {objectName: \"DOCUSIGN_HMAC_KEY\", key: \"DOCUSIGN_HMAC_KEY\"}\n",
        );
        let declined = BTreeSet::from(["DOCUSIGN_HMAC_KEY".to_string()]);
        let error = without_projected_objects(&unremovable, &declined)
            .expect_err("a surviving reference must abort the render");
        assert!(
            error.to_string().contains("DOCUSIGN_HMAC_KEY"),
            "the abort must name the object it could not remove: {error}"
        );
    }

    /// Render the GKE tree for `deployment` and return the object names its
    /// `SecretProviderClass` references after the per-deployment omission —
    /// the exact list the CSI driver would request at mount time.
    fn rendered_object_names(deployment: &str, env: &'static [(&str, &str)]) -> BTreeSet<String> {
        let subs = resolve_substitutions_for_deployment(deployment, "26.8.9", env_getter(env))
            .expect("env resolves");
        let rendered = render_manifests_with(&subs, false).expect("render succeeds");
        let gke = rendered.path().join(GKE_KUSTOMIZE_SUBPATH);
        let root = super::super::deployments::Deployment::load(&fixture_tree(), deployment)
            .expect("the deployment loads");
        let skipped =
            super::super::deployments::skipped_projected_objects(&root).unwrap_or_default();
        omit_unwritten_objects(&gke, &skipped).expect("the omission succeeds");
        let spc = fs::read_to_string(gke.join(SECRET_PROVIDER_CLASS))
            .expect("read the rendered SecretProviderClass");
        projected_object_names(&spc).expect("the rendered class parses")
    }

    /// The synthetic deployment tree these render tests read.
    ///
    /// The real rows moved to a private repository with the credential that
    /// rolls them. They were also both
    /// `provisioned = false`, so every assertion below was dormant — the
    /// omission is computed from a row's `secrets.enc.yaml` and an
    /// unprovisioned row has none. The fixture declares two provisioned rows,
    /// one per side of the two forks this seam turns on, and arms them. See
    /// `cli/tests/fixtures/deployment-tree/README.md`.
    fn fixture_tree() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("deployment-tree")
    }

    /// The row that declines `DocuSign` and does not own the webhook receiver.
    const ORDINARY_ROW: &str = "example-deployment";
    /// The row whose project id is `GITHUB_AUTOMATION_HOME_PROJECT`, declaring
    /// `DocuSign` and owning the receiver, so it skips nothing.
    const AUTOMATION_HOME_ROW: &str = "example-automation-home";

    #[test]
    fn the_rendered_class_references_exactly_what_the_deployment_writes() {
        // The invariant the whole seam exists for, asserted for every real
        // deployment: a CSI mount fails the entire volume on one object it
        // cannot read, so the class may reference an object only if
        // `ops secrets apply` writes it. Referencing fewer would leave a boot
        // requirement unprojected; referencing more is a pod that never starts.
        // Both fixture rows, because they skip different things: the ordinary
        // row omits DocuSign and the receiver trio, the automation home omits
        // nothing. One row could only ever prove one of those.
        for (deployment, env) in [(ORDINARY_ROW, HUB_ENV), (AUTOMATION_HOME_ROW, HUB_ENV)] {
            let root = super::super::deployments::Deployment::load(&fixture_tree(), deployment)
                .expect("the deployment loads");
            assert!(
                root.provisioned,
                "{deployment} must be provisioned or this assertion is dormant"
            );
            let skipped = super::super::deployments::skipped_projected_objects(&root)
                .expect("the tree is complete");
            let expected: BTreeSet<String> = super::super::deployments::projected_objects()
                .into_iter()
                .filter(|object| !skipped.contains(object))
                .collect();
            assert_eq!(
                rendered_object_names(deployment, env),
                expected,
                "{deployment}'s rendered class must reference exactly the objects it writes"
            );
        }
    }

    #[test]
    fn a_deployment_that_declines_docusign_renders_no_docusign_reference() {
        // A row that declares no `DOCUSIGN_BASE_URL` runs
        // `StubSignatureProvider`, holds no DocuSign object, and its mount must
        // not ask for one. This is the ship half of the failure: without it the
        // resolve preflight aborts naming all nine, and the only way past would
        // be a placeholder credential — which boots the real provider instead
        // of the stub.
        let referenced = rendered_object_names(ORDINARY_ROW, HUB_ENV);
        assert!(
            !referenced
                .iter()
                .any(|object| object.starts_with("DOCUSIGN_")),
            "a declined integration must leave no reference behind: {referenced:?}"
        );
        // The receiver's credentials belong to the automation home alone, and
        // a non-home row is forbidden to hold them at all.
        for scoped in [
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
            "NAVIGATOR_GITHUB_APP_LOGIN",
        ] {
            assert!(!referenced.contains(scoped), "{scoped} is scoped elsewhere");
        }
        // …while everything it does write is still projected.
        assert!(referenced.contains("NAVIGATOR_SURREAL_ENDPOINT"));
    }

    #[test]
    fn the_automation_home_keeps_every_reference() {
        // The other half: the automation home declares DocuSign and owns the
        // receiver, so nothing is omitted for it and its render is unchanged.
        //
        // This asked `neon-law-stg` while `neon-law-stg` is the automation
        // home, and passed because the row was unprovisioned and the test
        // returned early. Both halves are fixed together: the row it names is
        // now the one whose project id `GITHUB_AUTOMATION_HOME_PROJECT`
        // matches, and it is provisioned, so the assertion runs.
        let referenced = rendered_object_names(AUTOMATION_HOME_ROW, HUB_ENV);
        assert_eq!(referenced, super::super::deployments::projected_objects());
    }

    // ---------- the narrow `--image-only` lane ----------

    #[test]
    fn only_a_clean_diff_lets_the_image_only_lane_proceed() {
        // Exit 0 is the ONLY code that may continue. This is the property the
        // whole lane rests on: it applies no manifest change, so it is safe
        // exactly when there is no manifest change to apply.
        assert!(super::drift_verdict(Some(0)).is_ok());
    }

    #[test]
    fn manifest_drift_refuses_the_image_only_lane() {
        // Exit 1 means `kubectl diff` found a difference. The full roll treats
        // that as the normal signal to apply; this lane must treat it as
        // fatal, because it would move the images and silently leave the rest
        // of the diff unapplied. The inversion is the point.
        let error = super::drift_verdict(Some(1)).expect_err("drift is fatal here");
        let message = error.to_string();
        assert!(
            message.contains("not a version bump"),
            "the refusal must say why the lane is the wrong tool: {message}"
        );
        assert!(
            message.contains("ops ship"),
            "the refusal must name the roll that CAN apply the diff: {message}"
        );
    }

    #[test]
    fn an_unreachable_cluster_is_not_read_as_a_clean_diff() {
        // >1 is `kubectl diff` failing outright — unreachable context, auth
        // failure, kustomize build error. Nothing was compared, so nothing was
        // proven, and proceeding would ship on the strength of a check that
        // never ran. `None` is a signal kill: the same absence of an answer.
        for code in [Some(2), Some(127), None] {
            let error =
                super::drift_verdict(code).expect_err("a failed diff proves nothing about drift");
            assert!(
                error.to_string().contains("the cluster was not reached"),
                "exit {code:?} must read as an unreached cluster: {error}"
            );
        }
    }

    #[test]
    fn the_image_only_lane_never_guesses_a_tag() {
        // No `--tag` is a refusal, not a lookup of the latest published
        // release. The scheduled caller names the release it means.
        let error = super::image_only_tag(None).expect_err("--tag is required");
        assert!(
            error.to_string().contains("--image-only requires --tag"),
            "{error}"
        );
    }

    #[test]
    fn the_image_only_lane_rejects_a_tag_that_is_not_a_release() {
        // The scheduled caller derives its tag from a clock, so a derivation
        // that produces the wrong shape must fail here rather than as a pod
        // pulling an image nobody published.
        assert!(super::image_only_tag(Some("26.8.17")).is_ok());
        assert!(super::image_only_tag(Some("26.8.19-hotfix.14")).is_ok());
        for wrong in ["", "2026-08-17", "latest", "v26.8.17"] {
            assert!(
                super::image_only_tag(Some(wrong)).is_err(),
                "`{wrong}` is not a release tag"
            );
        }
    }

    #[test]
    fn the_narrowest_flag_picks_the_lane() {
        // `--image-only` is the lane an automated deploy runs under a
        // credential that can do nothing but bump a version, so it wins over a
        // stray `--restart-only` instead of silently widening into a restart.
        let roll = ShipOpts {
            deployment: "example-prod".into(),
            ..ShipOpts::default()
        };
        assert_eq!(ship_lane(&roll), ShipLane::Roll);
        assert_eq!(
            ship_lane(&ShipOpts {
                restart_only: true,
                ..roll.clone()
            }),
            ShipLane::RestartOnly
        );
        assert_eq!(
            ship_lane(&ShipOpts {
                image_only: true,
                ..roll.clone()
            }),
            ShipLane::ImageOnly
        );
        assert_eq!(
            ship_lane(&ShipOpts {
                image_only: true,
                restart_only: true,
                ..roll
            }),
            ShipLane::ImageOnly
        );
        assert!(lane_requires_asset_preflight(ShipLane::ImageOnly));
        assert!(lane_requires_asset_preflight(ShipLane::Roll));
        assert!(!lane_requires_asset_preflight(ShipLane::RestartOnly));
    }

    #[test]
    fn an_image_only_dry_run_walks_every_step_without_a_cluster() {
        // The whole lane under `--dry-run`: the context cross-check, the drift
        // refusal, both image writes, the `CronJob` re-pin, the rollout wait,
        // and the Restate re-register. Each one prints its command instead of
        // running it, so this proves the sequence is complete and
        // side-effect-free — a step that reached for kubectl, gcloud, the
        // deployment tree, or the network would fail right here.
        image_only_steps(&sample_config(), "26.8.17", true, Path::new("/nonexistent"))
            .expect("a dry-run image-only roll needs no cluster");
    }

    #[test]
    fn a_copied_context_aborts_the_image_only_lane_too() {
        // The narrow lane skips the manifest apply, not the guard that keeps a
        // stale shell from rolling one deployment onto another's cluster.
        let cfg = ShipConfig {
            project_id: "neon-law".into(),
            cluster: "acme-stg".into(),
            context: "gke_neon-law-stg_us-west4_neon-law-stg".into(),
            ..sample_config()
        };
        let err = image_only_steps(&cfg, "26.8.17", true, Path::new("/nonexistent"))
            .expect_err("a copied context must abort the narrow lane");
        assert!(err.to_string().contains("Shipping would roll"), "{err}");
    }

    #[test]
    fn image_only_pins_both_deployments_to_one_published_tag() {
        let writes = image_only_image_writes(&sample_config(), "26.8.17");
        assert_eq!(
            writes.iter().map(|(d, _)| *d).collect::<Vec<_>>(),
            vec![WEB_DEPLOYMENT, WORKFLOWS_DEPLOYMENT],
            "both service deployments move, never one"
        );
        for (deployment, image) in &writes {
            assert!(
                image.starts_with("ghcr.io/neon-law-source-code/"),
                "{deployment} pulls the published image: {image}"
            );
            assert!(
                image.ends_with(":26.8.17"),
                "{deployment} lands on the named release with no skew: {image}"
            );
        }
    }

    #[test]
    fn a_dry_run_reads_no_cluster_to_decide_drift() {
        // Dry-run renders nothing and reads nothing, so the diff never runs —
        // which is why a root that does not exist is still fine here. Reading
        // the lane back before running it must not itself render the tree or
        // touch the cluster.
        refuse_on_manifest_drift(
            &sample_config(),
            "26.8.17",
            true,
            Path::new("/nonexistent-deployment-tree"),
        )
        .expect("a dry-run drift check needs no cluster");
    }

    #[test]
    fn a_restart_only_dry_run_walks_its_steps_without_a_cluster() {
        // The sibling lane behind the same seam: rollout restart, then wait.
        restart_only_steps(&sample_config(), true).expect("a dry-run restart needs no cluster");
    }
}
