//! The deployment tree: plaintext coordinates beside SOPS-encrypted key
//! material.
//!
//! ```text
//! deployments/<name>/config.toml       coordinates, plaintext, reviewable
//! deployments/<name>/secrets.enc.yaml  key material, SOPS, per-deployment KMS key
//! .sops.yaml                           which key encrypts which deployment
//! ```
//!
//! ## Where the tree is
//!
//! **Not in this repository.** The real rows live in a private repository,
//! beside the workflow that rolls them, which this one does not name.
//! This one is public and accepts pull requests from strangers, so a workflow
//! here holding a credential that reaches the cluster is an attack surface the
//! split removes; the tree moved because the workflow needs it on disk, not
//! because per-value KMS ciphertext needed hiding.
//!
//! [`root`] is how a command finds it: `--deployments-dir`, then
//! `NAVIGATOR_DEPLOYMENTS_DIR`, then the workspace. The workspace fallback is
//! what the synthetic tree under `cli/tests/fixtures/deployment-tree/` is NOT
//! — the fixture is passed explicitly, and it exists so the gates below keep
//! running here. See its README.
//!
//! ## What this module will and will not read
//!
//! Two entry points, deliberately asymmetric. [`Deployment::load`] reads the
//! plaintext coordinates and the *key names* of the encrypted file — never a
//! value, never a `sops` invocation, no KMS call, no credential. Every audit,
//! parity check, and `--dry-run` runs on that. Only [`apply`] decrypts, and
//! only after the names-only check has already passed.
//!
//! ## Why `sops` is a shell-out
//!
//! `sops` is invoked the way this CLI already invokes `gcloud`, `kubectl`,
//! `helm`, and `docker`. The Rust-owns-the-workspace invariant governs
//! Navigator's own automation, not every dependency it drives. The Rust
//! reimplementation `rops` was rejected on a concrete capability gap rather
//! than on maturity: it implements age and AWS KMS only, so it cannot express
//! the per-deployment Cloud KMS keys this design is built on.
//!
//! ## Rotation
//!
//! Re-encrypting a file rotates the data key. It revokes nothing: every prior
//! ciphertext stays readable to anyone holding repo history and the KMS key,
//! because the key decrypts history and not just `HEAD`. **A rotation is a
//! rotation at the provider, followed by re-encrypting here.** The file edit
//! alone is a no-op against an attacker who already has the old bytes. See
//! `docs/deployment-secrets.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use store::deployment::{applicable_web_requirements, Requirement};
use store::DeploymentEnvironment;

use super::gcp;
use super::ship;

/// The tree's root, relative to the workspace.
pub const TREE: &str = "deployments";
/// Plaintext coordinates. Reviewable in a diff, greppable, CI-checkable.
pub const CONFIG_FILE: &str = "config.toml";
/// Key material, encrypted per value against this deployment's own KMS key.
pub const SECRETS_FILE: &str = "secrets.enc.yaml";
/// The creation rules deciding which KMS key encrypts which deployment. Sits
/// beside the tree rather than inside it, because a rule's `path_regex` is
/// written relative to the directory `sops` runs from.
pub const SOPS_CONFIG: &str = ".sops.yaml";

/// Names the directory the tree lives in, for a checkout that is not the
/// workspace. Read by [`root`] when no explicit path is passed.
pub const ROOT_ENV: &str = "NAVIGATOR_DEPLOYMENTS_DIR";

/// The coordinate every other coordinate is scoped by.
const PROJECT_ID: &str = "NAVIGATOR_GCP_PROJECT_ID";

/// `sops` marks each encrypted scalar with this prefix. A value that does not
/// carry it is plaintext sitting in the repository.
const ENCRYPTED_PREFIX: &str = "ENC[";
/// The metadata block `sops` appends to every file it encrypts.
const SOPS_METADATA_KEY: &str = "sops";

#[derive(Debug, Deserialize)]
struct ConfigFile {
    kms_key: String,
    #[serde(default = "provisioned_by_default")]
    provisioned: bool,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// A deployment is provisioned unless it says otherwise.
///
/// The default is the safe one: forgetting the key on a real row makes it
/// eligible for every gate, where forgetting it on an unprovisioned row would
/// silently exempt it.
const fn provisioned_by_default() -> bool {
    true
}

/// One deployment as the repository describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub name: String,
    /// The Cloud KMS key `sops` encrypts this deployment's key material
    /// against, in this deployment's own project.
    pub kms_key: String,
    /// Whether the cloud resources this row describes actually exist yet.
    ///
    /// `false` means the tree describes a deployment nobody has run
    /// `ops gcp setup` for: its project, KMS key, buckets, OAuth client, and
    /// Surreal instance are all still to be created, so its coordinates are
    /// blank and it has no `secrets.enc.yaml` to encrypt against a key that
    /// does not exist.
    ///
    /// This is a declared state rather than an inferred one. A row with empty
    /// coordinates is indistinguishable from a row somebody broke, and the
    /// completeness gates cannot tell the difference on their own — so an
    /// unprovisioned row says so in one greppable line, the gates skip exactly
    /// those rows, and `ops ship` refuses one outright rather than failing
    /// somewhere deep in a `kubectl apply`.
    pub provisioned: bool,
    /// Plaintext, non-secret deployment coordinates.
    pub coordinates: BTreeMap<String, String>,
    /// The key *names* the encrypted file carries. A `BTreeSet<String>` and
    /// not a map on purpose: there is nowhere for a value to live, so no
    /// audit path can print one even by mistake.
    pub encrypted_keys: BTreeSet<String>,
}

impl Deployment {
    /// Read `<root>/deployments/<name>/`. Fails closed on a missing file, a
    /// missing project coordinate, or an encrypted file that is not encrypted.
    pub fn load(root: &Path, name: &str) -> Result<Self> {
        let dir = root.join(TREE).join(name);
        if !dir.is_dir() {
            bail!(
                "no deployment `{name}` in {}/. Known deployments: {}",
                TREE,
                names(root)?.join(", ")
            );
        }
        let config_path = dir.join(CONFIG_FILE);
        let config: ConfigFile = toml::from_str(
            &fs::read_to_string(&config_path)
                .with_context(|| format!("read {}", config_path.display()))?,
        )
        .with_context(|| format!("parse {}", config_path.display()))?;

        if !config.env.contains_key(PROJECT_ID) {
            bail!(
                "{} must set {PROJECT_ID}: every coordinate and every Secret Manager write is \
                 scoped by the deployment's own project",
                config_path.display()
            );
        }

        // An unprovisioned row has no `secrets.enc.yaml`, and cannot: `sops`
        // encrypts against a KMS key that does not exist yet. Reading it as an
        // empty set is the honest answer — the row genuinely supplies no key
        // material — and every gate that cares checks `provisioned` first.
        let secrets_path = dir.join(SECRETS_FILE);
        let encrypted_keys = match fs::read_to_string(&secrets_path) {
            Ok(body) => encrypted_key_names(&body)
                .with_context(|| format!("read the key names of {}", secrets_path.display()))?,
            Err(error) if !config.provisioned && error.kind() == std::io::ErrorKind::NotFound => {
                BTreeSet::new()
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", secrets_path.display()))
            }
        };

        Ok(Self {
            name: name.to_owned(),
            kms_key: config.kms_key,
            provisioned: config.provisioned,
            coordinates: config.env,
            encrypted_keys,
        })
    }

    /// The GCP project that owns this deployment's Secret Manager and KMS key.
    #[must_use]
    pub fn project_id(&self) -> &str {
        self.coordinates
            .get(PROJECT_ID)
            .expect("load rejects a config without a project id")
    }

    /// Every key name this deployment supplies to a running pod: plaintext
    /// coordinates, encrypted key material, and the keys the Deployment
    /// manifest supplies as inline env rather than through the Secret.
    #[must_use]
    pub fn supplied_keys(&self) -> BTreeSet<String> {
        self.coordinates
            .keys()
            .chain(self.encrypted_keys.iter())
            .cloned()
            .chain(ship::INLINE_ENV_WEB_KEYS.iter().map(ToString::to_string))
            .collect()
    }
}

/// The directory that CONTAINS `deployments/` — which is also the directory
/// `.sops.yaml` sits in, because a creation rule's `path_regex` is written
/// relative to it.
///
/// Three sources, in priority order: an explicit `--deployments-dir`, the
/// `NAVIGATOR_DEPLOYMENTS_DIR` environment variable, and the discovered
/// workspace root. The first two exist because the tree does not have to live
/// beside the source that reads it. `orchestrate::workspace_root` walks up for
/// a directory holding both `Cargo.toml` and `k8s/`; a checkout that carries
/// the deployment tree and nothing else has neither, so without an override
/// every read of the tree bails there rather than at the tree.
///
/// Nothing is inferred from the process environment beyond that one variable,
/// and the variable names a *location*, never a deployment — `--deployment`
/// stays explicit for the same reason it always has.
pub fn root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return holding_the_tree(dir, "--deployments-dir");
    }
    if let Some(value) = env::var_os(ROOT_ENV) {
        return holding_the_tree(Path::new(&value), ROOT_ENV);
    }
    let workspace = super::orchestrate::workspace_root()?;
    if workspace.join(TREE).is_dir() {
        return Ok(workspace);
    }
    // The ordinary case now, not an edge one: this repository is public and
    // holds no deployment tree. Falling through to a bare `no deployment
    // <name>` from `Deployment::load` would describe the wrong problem, and
    // `names` would fail on a `read_dir` of a path that does not exist.
    bail!(
        "no `{TREE}/` in the workspace at {}. The deployment tree lives in a private \
         repository, beside the credential that rolls it — run this from a checkout of that \
         repository, or pass `--deployments-dir` (or set {ROOT_ENV}) to point at one",
        workspace.display()
    )
}

/// Accept `dir` only if the tree is actually under it.
///
/// The failure this spends a branch on is being handed the tree itself. It is
/// the reading the flag's name invites, it produces a `no deployment <name> in
/// deployments/` error that describes the wrong problem, and the fix — pass
/// the parent — is not derivable from that message.
fn holding_the_tree(dir: &Path, source: &str) -> Result<PathBuf> {
    if dir.join(TREE).is_dir() {
        return Ok(dir.to_path_buf());
    }
    if dir.file_name().is_some_and(|name| name == TREE) {
        let parent = dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| ".".to_owned(), |parent| parent.display().to_string());
        bail!(
            "{source} is {}, the `{TREE}` tree itself. It wants the directory CONTAINING the \
             tree, which is also where `.sops.yaml` sits: pass {parent} instead",
            dir.display()
        );
    }
    bail!(
        "{source} is {}, which has no `{TREE}/` directory in it",
        dir.display()
    )
}

/// Every deployment directory in the tree, sorted.
pub fn names(root: &Path) -> Result<Vec<String>> {
    let tree = root.join(TREE);
    let mut names = Vec::new();
    for entry in
        fs::read_dir(&tree).with_context(|| format!("read the tree at {}", tree.display()))?
    {
        let entry = entry.context("read a deployment directory entry")?;
        if entry.path().is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The key names of a SOPS-encrypted YAML document, and the guard that a
/// plaintext value can never be committed as one.
///
/// Three things must hold, and each has its own failure mode:
///
/// 1. The `sops` metadata block is present. Without it the file was never
///    encrypted at all — the accident of writing values and forgetting to run
///    `sops --encrypt`.
/// 2. Every value is `ENC[…]`. This is the per-value encryption the design
///    turns on: a whole-file envelope would produce a single `data` blob
///    instead, which makes every rotation an opaque one-line diff.
/// 3. Every key is an environment-variable name. A blob file's `data` key
///    fails this too, so the two guards reinforce each other.
pub fn encrypted_key_names(document: &str) -> Result<BTreeSet<String>> {
    let parsed: BTreeMap<String, serde_yaml::Value> =
        serde_yaml::from_str(document).context("parse the encrypted YAML document")?;

    let mut names = BTreeSet::new();
    let mut has_metadata = false;
    for (key, value) in parsed {
        if key == SOPS_METADATA_KEY {
            has_metadata = true;
            continue;
        }
        if !is_env_var_name(&key) {
            bail!(
                "`{key}` is not an environment-variable name. Key material is encrypted per \
                 value, one entry per variable; a whole-file envelope (a single `data` blob) \
                 makes every rotation an opaque one-line diff"
            );
        }
        let encrypted = value
            .as_str()
            .is_some_and(|value| value.starts_with(ENCRYPTED_PREFIX));
        if !encrypted {
            bail!(
                "`{key}` is not encrypted: its value does not start with `{ENCRYPTED_PREFIX}`. \
                 Run `sops --encrypt --in-place` before committing — a plaintext secret in the \
                 repository cannot be un-committed, only rotated at the provider"
            );
        }
        names.insert(key);
    }

    if !has_metadata {
        bail!(
            "the document carries no `{SOPS_METADATA_KEY}` metadata block, so it was never \
             encrypted"
        );
    }
    Ok(names)
}

/// `^[A-Z][A-Z0-9_]*$` — the same shape `.sops.yaml`'s `encrypted_regex`
/// selects values by, so a key this refuses is also a key `sops` would have
/// left in plaintext.
fn is_env_var_name(key: &str) -> bool {
    let mut characters = key.chars();
    characters.next().is_some_and(|c| c.is_ascii_uppercase())
        && characters.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The boot requirements this deployment does not satisfy.
///
/// Not "every deployment carries every key": `applicable_web_requirements`
/// scopes a requirement to one project when it names one, and the GitHub
/// webhook five-tuple is scoped to the automation home alone. A deployment is
/// measured against the requirements that apply *to it*.
///
/// Names only. Nothing here decrypts, so this runs in CI with no credential
/// and no network call.
#[must_use]
pub fn unsatisfied_requirements(deployment: &Deployment) -> Vec<Requirement> {
    let supplied = deployment.supplied_keys();
    let get = |key: &str| -> Option<String> {
        // A coordinate answers with its real value — `NAVIGATOR_GCP_PROJECT_ID`
        // decides which project-scoped requirements apply. Everything else
        // answers with presence: the file's key names are all this may see,
        // and `encrypted_key_names` has already refused an empty one.
        deployment
            .coordinates
            .get(key)
            .cloned()
            .or_else(|| supplied.contains(key).then(|| "1".to_owned()))
    };

    // Every hosted deployment runs the production profile, so the CI-harness
    // relaxation cannot apply and every integration-tier requirement counts.
    applicable_web_requirements(DeploymentEnvironment::Production, &get)
        .into_iter()
        .filter(|requirement| {
            !requirement
                .any_of
                .iter()
                .any(|alternative| alternative.iter().all(|key| supplied.contains(*key)))
        })
        .collect()
}

/// Render one unsatisfied requirement as the key names that would satisfy it.
#[must_use]
pub fn describe(requirement: &Requirement) -> String {
    requirement
        .any_of
        .iter()
        .map(|alternative| alternative.join(" + "))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The Secret Manager objects a deployment must populate: every key the
/// shipped `SecretProviderClass` projects into the pod's Secret.
#[must_use]
pub fn projected_objects() -> BTreeSet<String> {
    ship::secret_provider_class_keys()
}

/// Where each projected object's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Coordinate,
    Encrypted,
}

/// Why an object the `SecretProviderClass` references is legitimately absent
/// from one deployment's tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exemption {
    /// The object belongs to a deployment other than this one. The
    /// `SecretProviderClass` is one embedded manifest rendered for every
    /// deployment, so it references the engineering webhook receiver's
    /// credentials everywhere — while `store::deployment` says that receiver
    /// runs in exactly one project and no other deployment may hold them.
    /// Demanding them would push a singleton's HMAC secret into a production
    /// row that must not have it.
    ScopedElsewhere,
    /// The object is one alternative of a requirement this deployment already
    /// satisfies another way. `DOCUSIGN_ACCESS_TOKEN` is the case in hand: it
    /// is an alternative to the `DOCUSIGN_INTEGRATION_KEY` + `_USER_ID` +
    /// `_PRIVATE_KEY` JWT triple, so a deployment using JWT auth correctly has
    /// no access token and the manifest over-specifies by naming both.
    AlternativeSatisfied,
    /// The object belongs to an integration this deployment declines outright.
    /// A `Requirement` carrying a `trigger` is demanded only once that trigger
    /// key is supplied, so a deployment supplying none of the integration has
    /// nothing to satisfy — and must not be handed a placeholder to get past
    /// this check. `neon-law-stg` declines `DocuSign` (no `DOCUSIGN_BASE_URL`)
    /// and runs `StubSignatureProvider`, which `portal::signature` reaches only
    /// through genuine absence.
    Untriggered,
}

impl Exemption {
    const fn reason(self) -> &'static str {
        match self {
            Self::ScopedElsewhere => "scoped to another deployment",
            Self::AlternativeSatisfied => "requirement satisfied another way",
            Self::Untriggered => "integration not declared by this deployment",
        }
    }
}

/// Whether `object` is some requirement's trigger key that this deployment does
/// not supply.
///
/// A trigger is optional by construction — supplying it is what turns the
/// integration on — so it is never itself a requirement and never appears in an
/// `any_of`. Without this, the trigger would be the one object of a declined
/// integration that no rule could exempt.
fn is_undeclared_trigger(object: &str, supplied: &BTreeSet<String>) -> bool {
    store::deployment::WEB_REQUIREMENTS
        .iter()
        .filter_map(|requirement| requirement.trigger)
        .any(|trigger| trigger == object && !supplied.contains(trigger))
}

/// Whether this deployment may legitimately omit `object`.
///
/// Derived from `WEB_REQUIREMENTS` itself — its `project_id` scoping, its
/// `trigger`, and its `any_of` alternatives — rather than a second
/// hand-maintained list, because a hand-maintained copy is the drift this whole
/// tree exists to remove. The three filters are exactly the ones
/// `store::deployment::applicable_web_requirements` applies when `web` enforces
/// the same table at boot; reading fewer of them here is what made a declined
/// integration look like a missing credential. An object no requirement
/// mentions at all is never exempt: a Drive folder id is projected everywhere on
/// purpose, and its absence is a real gap.
fn exemption(object: &str, project_id: &str, supplied: &BTreeSet<String>) -> Option<Exemption> {
    if is_undeclared_trigger(object, supplied) {
        return Some(Exemption::Untriggered);
    }
    let mut verdict = None;
    for requirement in store::deployment::WEB_REQUIREMENTS {
        if !requirement
            .any_of
            .iter()
            .any(|alternative| alternative.contains(&object))
        {
            continue;
        }
        let scoped_elsewhere = requirement
            .project_id
            .is_some_and(|scope| scope != project_id);
        let untriggered = requirement
            .trigger
            .is_some_and(|trigger| !supplied.contains(trigger));
        let satisfied_otherwise = requirement.any_of.iter().any(|alternative| {
            !alternative.contains(&object) && alternative.iter().all(|key| supplied.contains(*key))
        });
        let reason = if scoped_elsewhere {
            Exemption::ScopedElsewhere
        } else if untriggered {
            Exemption::Untriggered
        } else if satisfied_otherwise {
            Exemption::AlternativeSatisfied
        } else {
            // One requirement genuinely wants this object here, which settles
            // it however the others read.
            return None;
        };
        verdict = Some(reason);
    }
    verdict
}

/// The projected objects this deployment legitimately does not write — the set
/// the rendered `SecretProviderClass` must not reference.
///
/// Read straight off [`plan`], so the rendered object list and the objects
/// `ops secrets apply` writes cannot drift: what the class references is exactly
/// what the deployment supplies. That symmetry is the whole property. A CSI
/// mount fails the entire volume on one object it cannot read, so an entry the
/// deployment will never write is not a harmless reference — it is a pod that
/// never starts, and a ship that aborts at the resolve preflight before it.
///
/// # Errors
///
/// Propagates [`plan`]'s failure when an object is supplied by neither file and
/// no rule exempts it: a real gap in the deployment's tree, not something to
/// quietly omit from the mount.
pub fn skipped_projected_objects(deployment: &Deployment) -> Result<BTreeSet<String>> {
    let (_, skipped) = plan(deployment)?;
    Ok(skipped.into_keys().collect())
}

/// `navigator ops deployments check --deployments-dir <dir>` — run every
/// tree-level gate against whatever tree it is pointed at.
///
/// The workspace suite asserts these against the synthetic tree in
/// `cli/tests/fixtures/deployment-tree/`, which is all this repository has: the
/// real rows moved to a private repository. A gate that only ever runs against a
/// fixture proves the fixture, so the same assertions ship as a command the
/// tree's own CI runs.
///
/// Four things, and each has caught something:
///
/// 1. Every row loads — the config parses, names its project, and its
///    `secrets.enc.yaml` is per-value encrypted rather than a plaintext file
///    somebody forgot to run `sops` over.
/// 2. No stray decrypted file sits beside it. A `secrets.yaml` would not be
///    loaded at all, so loading is not enough; the directory is walked.
/// 3. Every `.sops.yaml` rule agrees with the `kms_key` its row declares, and
///    that key lives in that row's own project. Disagreement means one
///    deployment's key material is encrypted against another's — the isolation
///    failing silently, because both still decrypt for whoever holds both.
/// 4. Every boot requirement that applies to a row is satisfied by that row's
///    own files, and every object the `SecretProviderClass` projects is
///    supplied or explicitly skipped.
///
/// Names only. Nothing here decrypts, so it runs in CI with no KMS grant, no
/// credential, and no network — the same asymmetry [`Deployment::load`] draws.
pub fn check(root: &Path) -> Result<()> {
    let names = names(root)?;
    if names.is_empty() {
        bail!(
            "{}/{TREE} describes no deployment. An empty tree passes every check below \
             vacuously, which is indistinguishable from a healthy one",
            root.display()
        );
    }

    let sops_rules = fs::read_to_string(root.join(SOPS_CONFIG))
        .with_context(|| format!("read {}/{SOPS_CONFIG}", root.display()))?;

    let mut checked = 0_usize;
    for name in &names {
        let deployment = Deployment::load(root, name)?;
        no_stray_plaintext(root, name)?;

        if !sops_rules.contains(&deployment.kms_key) {
            bail!(
                "no {SOPS_CONFIG} creation rule names {name}'s key {}, so the next `sops` write \
                 to that row encrypts against some other rule's key",
                deployment.kms_key
            );
        }
        if !deployment.kms_key.contains(deployment.project_id()) {
            bail!(
                "{name}'s key must live in its own project ({}), or the per-deployment isolation \
                 is nominal: {}",
                deployment.project_id(),
                deployment.kms_key
            );
        }

        // An unprovisioned row describes resources nobody has created yet, so
        // its coordinates are deliberately blank and it carries no encrypted
        // file. Demanding completeness of it would only force placeholder
        // values into the tree that read as real.
        if !deployment.provisioned {
            eprintln!("{name}: declared unprovisioned, completeness not checked");
            continue;
        }

        let unsatisfied = unsatisfied_requirements(&deployment);
        if !unsatisfied.is_empty() {
            bail!(
                "{name} does not satisfy {} boot requirement(s) that apply to it: {}",
                unsatisfied.len(),
                unsatisfied
                    .iter()
                    .map(describe)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let (plan, skipped) = plan(&deployment)?;
        eprintln!(
            "{name}: {} object(s) supplied, {} skipped{}",
            plan.len(),
            skipped.len(),
            if skipped.is_empty() {
                String::new()
            } else {
                format!(
                    " ({})",
                    skipped
                        .iter()
                        .map(|(object, reason)| format!("{object}: {}", reason.reason()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        );
        checked += 1;
    }

    eprintln!(
        "checked {} deployment(s) in {}/{TREE}, {checked} provisioned",
        names.len(),
        root.display()
    );
    Ok(())
}

/// A decrypted working copy must never sit beside the encrypted one.
///
/// `Deployment::load` refuses a plaintext value inside `secrets.enc.yaml`, but
/// a stray `secrets.yaml` is a different file and would not be loaded at all.
/// The directory is walked so the accident is caught by its filename.
fn no_stray_plaintext(root: &Path, name: &str) -> Result<()> {
    let dir = root.join(TREE).join(name);
    for entry in
        fs::read_dir(&dir).with_context(|| format!("read the tree at {}", dir.display()))?
    {
        let path = entry.context("read a deployment directory entry")?.path();
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if file_name == CONFIG_FILE || file_name == SECRETS_FILE {
            continue;
        }
        bail!(
            "{TREE}/{name}/{file_name} is neither the plaintext coordinates nor the encrypted key \
             material. A decrypted file must never be written here: committing one cannot be \
             undone, only rotated at the provider"
        );
    }
    Ok(())
}

/// Resolve every projected object to the file that supplies it, or fail
/// naming the objects nothing supplies. Names only — no value is read here.
///
/// Returns the plan and the objects deliberately skipped, so the caller can
/// report a skip by name. A skip is never silent: an operator reading the
/// output must be able to see that this deployment writes fewer objects than
/// the manifest references, and why.
fn plan(
    deployment: &Deployment,
) -> Result<(BTreeMap<String, Source>, BTreeMap<String, Exemption>)> {
    let project_id = deployment.project_id();
    let supplied = deployment.supplied_keys();
    let mut plan = BTreeMap::new();
    let mut skipped = BTreeMap::new();
    let mut missing = Vec::new();
    for object in projected_objects() {
        if deployment.encrypted_keys.contains(&object) {
            plan.insert(object, Source::Encrypted);
        } else if deployment.coordinates.contains_key(&object) {
            plan.insert(object, Source::Coordinate);
        } else if let Some(reason) = exemption(&object, project_id, &supplied) {
            skipped.insert(object, reason);
        } else {
            missing.push(object);
        }
    }
    if !missing.is_empty() {
        bail!(
            "{} object(s) the SecretProviderClass projects are in neither {CONFIG_FILE} nor \
             {SECRETS_FILE} for {}: {}. Secret Manager was not changed.",
            missing.len(),
            deployment.name,
            missing.join(", ")
        );
    }
    Ok((plan, skipped))
}

/// `navigator ops secrets apply --deployment <name>` — decrypt this
/// deployment's key material and write `versions/latest` for every object the
/// `SecretProviderClass` projects, in that deployment's own project.
///
/// `dry_run` returns after the names-only plan. It does not shell out to
/// `sops`, so it needs no KMS decrypt permission and produces no plaintext to
/// leak — the point of a dry run is to check the shape, and the shape is the
/// names.
pub fn apply(root: &Path, name: &str, dry_run: bool) -> Result<()> {
    let deployment = Deployment::load(root, name)?;

    // Boot invariants before Secret Manager objects: a deployment missing a
    // requirement that applies to it will crash-loop on the values this
    // command is about to write, and finding that out here costs nothing.
    let unsatisfied = unsatisfied_requirements(&deployment);
    if !unsatisfied.is_empty() {
        bail!(
            "{name} does not satisfy {} boot requirement(s) that apply to it: {}. Secret Manager \
             was not changed.",
            unsatisfied.len(),
            unsatisfied
                .iter()
                .map(describe)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let (plan, skipped) = plan(&deployment)?;
    let project_id = deployment.project_id().to_owned();

    eprintln!(
        "secrets apply: deployment={name} project={project_id} object(s)={}",
        plan.len()
    );
    eprintln!(
        "from {CONFIG_FILE}: {}",
        join_names(&plan, Source::Coordinate)
    );
    eprintln!(
        "from {SECRETS_FILE}: {}",
        join_names(&plan, Source::Encrypted)
    );
    if !skipped.is_empty() {
        eprintln!(
            "skipped {} object(s) this deployment may legitimately omit: {}",
            skipped.len(),
            skipped
                .iter()
                .map(|(object, reason)| format!("{object} ({})", reason.reason()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if dry_run {
        eprintln!("DRY-RUN: nothing was decrypted and Secret Manager was not changed");
        return Ok(());
    }

    let decrypted = decrypt(&root.join(TREE).join(name).join(SECRETS_FILE))?;
    let mut payloads = BTreeMap::new();
    for (object, source) in &plan {
        let value = match source {
            Source::Encrypted => decrypted.get(object).cloned(),
            Source::Coordinate => deployment.coordinates.get(object).cloned(),
        };
        let value = value.filter(|value| !value.is_empty()).with_context(|| {
            format!("{object} resolved to an empty value; Secret Manager was not changed")
        })?;
        payloads.insert(object.clone(), value);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    runtime.block_on(async move {
        let token = gcp::auth::adc_token_provider().await?;
        // Never a dry-run client: its recorder serializes the request body,
        // which is where the payload rides.
        let client = gcp::client::GcpClient::new(token);
        for (object, value) in &payloads {
            gcp::secret_manager::ensure_secret(&client, &project_id, object).await?;
            gcp::secret_manager::add_version(&client, &project_id, object, value.as_bytes())
                .await?;
            eprintln!("wrote a new version of {object}");
        }
        eprintln!(
            "applied {} object(s) to projects/{project_id}/secrets",
            payloads.len()
        );
        Ok::<(), anyhow::Error>(())
    })
}

fn join_names(plan: &BTreeMap<String, Source>, source: Source) -> String {
    plan.iter()
        .filter(|(_, entry)| **entry == source)
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Decrypt one deployment's key material through `sops`.
///
/// The plaintext lands in this process's memory and nowhere else: `sops`
/// writes it to a pipe, not to disk, and the values never reach `argv`, a log
/// line, or an error message. A failure surfaces `sops`'s own stderr, which
/// describes the KMS call rather than the file's contents.
fn decrypt(path: &Path) -> Result<BTreeMap<String, String>> {
    let output = Command::new("sops")
        .arg("--decrypt")
        .arg("--output-type")
        .arg("json")
        .arg(path)
        .output()
        .with_context(|| {
            format!(
                "run `sops --decrypt {}`. Install sops (`brew install sops`) and authenticate \
                 with a principal holding cloudkms.cryptoKeyDecrypter on this deployment's key.",
                path.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "sops could not decrypt {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        // No `with_context` carrying the body: a parse error's payload is the
        // decrypted document.
        .map_err(|error| anyhow::anyhow!("parse the decrypted document: {error}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use store::deployment::GITHUB_AUTOMATION_HOME_PROJECT;

    use super::*;

    /// Every deployment in the tree that declares itself provisioned.
    ///
    /// The secret-projection guards below exercise a real row's real
    /// `secrets.enc.yaml`, so they have nothing to check on a row whose cloud
    /// resources do not exist yet — it carries no encrypted file at all.
    ///
    /// **The tree currently declares none**, so those guards are dormant: they
    /// pass without asserting anything until the first `provisioned = true`
    /// lands. That is stated here rather than hidden behind a silent skip,
    /// because a dormant guard reads exactly like a passing one in CI. The
    /// commit that provisions a deployment re-arms every one of them, and any
    /// drift they would have caught surfaces then.
    fn provisioned_deployments(root: &Path) -> Vec<Deployment> {
        names(root)
            .expect("the deployments tree is readable")
            .into_iter()
            .map(|name| Deployment::load(root, &name).expect("the deployment loads"))
            .filter(|deployment| deployment.provisioned)
            .collect()
    }

    /// The workspace checkout, for the tests that read the repository's own
    /// files. It no longer holds a deployment tree — see [`fixture_tree`].
    fn workspace() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the cli crate has a workspace parent")
            .to_path_buf()
    }

    /// The synthetic tree these gates read, and the only one this repository
    /// has.
    ///
    /// The real rows moved to a private repository with the credential that
    /// rolls them, so a workspace test can no longer
    /// read a shipping deployment. The fixture is not a weaker substitute: both
    /// real rows declare `provisioned = false`, and every gate below skips an
    /// unprovisioned row, so they were dormant — passing without asserting
    /// anything, which reads exactly like passing. The fixture declares two
    /// provisioned rows and arms them.
    ///
    /// `cli/tests/fixtures/deployment-tree/README.md` says what each row is
    /// for. The real rows are gated by `navigator ops deployments check`, which
    /// runs these same assertions against whatever tree it is pointed at.
    fn fixture_tree() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("deployment-tree")
    }

    /// Every literal `--deployment <name>` argument in `text`, as an operator
    /// would copy it. Placeholders (`<name>`, `$NAME`, `{name}`) and prose
    /// mentioning the flag are not instructions and are skipped: an argument
    /// counts only when it has the tree's `<word>-<word>` directory shape.
    fn instructed_deployments(text: &str) -> Vec<String> {
        let is_name = |token: &str| {
            token.contains('-')
                && !token.starts_with('-')
                && token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        let mut instructed = Vec::new();
        for (index, _) in text.match_indices("--deployment") {
            let tail = &text[index + "--deployment".len()..];
            // `--deployments-dir` starts with `--deployment` and is a different
            // flag taking a path. Without this the scan reads its tail as a
            // deployment called `s-dir`, which is a name no tree will ever hold
            // — a false positive that fails the build on a correct document.
            if !tail.starts_with(['=', ' ', '\t', '\n', '\r', '"', '\'']) && !tail.is_empty() {
                continue;
            }
            let tail = tail.strip_prefix('=').unwrap_or(tail);
            if tail.trim_start().starts_with(['<', '$', '{']) {
                continue;
            }
            let Some(token) = tail.split_whitespace().next() else {
                continue;
            };
            let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if is_name(token) {
                instructed.push(token.to_string());
            }
        }
        instructed
    }

    /// No public source instructs a roll by name, because this repository
    /// cannot know whether the name is still real.
    ///
    /// The rule used to be the other way round: a workflow or runbook could
    /// name any deployment whose `deployments/<name>/` directory existed, and
    /// this test read the tree to check. The tree moved to a private
    /// repository, so there is nothing here to
    /// check a name against — and a name that cannot be checked is exactly the
    /// failure the old rule existed to prevent. It once left an operator
    /// instructed to roll `neon-law-stg` from a config that did not exist;
    /// hardcoding names now would reintroduce that with no gate at all.
    ///
    /// So every public instruction takes a placeholder, and the hand-off points
    /// at a checkout rather than a row. `deployments/<name>/` appearing in prose
    /// is fine — the flag ARGUMENT is what an operator copies, and
    /// `instructed_deployments` only counts that. Which repository holds the
    /// tree is a separate question, covered by
    /// `the_ship_handoff_takes_the_deploy_repository_from_configuration`.
    #[test]
    fn no_public_source_instructs_a_deployment_by_name() {
        let root = workspace();
        let sources = [
            ".github/workflows/deploy.yml",
            "docs/gitops.md",
            "docs/gke-prod.md",
            "docs/cronjobs.md",
            "docs/environments.md",
            "docs/deployment-secrets.md",
            "docs/deploy/gke-ship-example.md",
            "server/content/workshops/navigator/DEPLOY.md",
        ];
        for source in sources {
            let text = fs::read_to_string(root.join(source))
                .unwrap_or_else(|error| panic!("read {source}: {error}"));
            let instructed = instructed_deployments(&text);
            assert!(
                instructed.is_empty(),
                "{source} instructs `--deployment {}`, but this repository holds no deployment \
                 tree to make that name real. Write a placeholder — `<row>`, `$NAME` — and let \
                 the deploy repository be the thing that knows which rows exist.",
                instructed.join("`, `--deployment ")
            );
        }

        // And the hand-off must not try to enumerate a tree that is not here.
        // It once ran `ls deployments` and read each row's public host out of
        // its config.toml; both would now fail at run time, inside a Slack
        // message step whose failure nobody reads as "the tree moved".
        let workflow = ".github/workflows/deploy.yml";
        let text = fs::read_to_string(root.join(workflow))
            .unwrap_or_else(|error| panic!("read {workflow}: {error}"));
        for gone in ["$(ls deployments", "deployments/${name}/config.toml"] {
            assert!(
                !text.contains(gone),
                "{workflow} still reads the deployment tree (`{gone}`), which this repository no \
                 longer has"
            );
        }
    }

    /// The ship hand-off points at the deploy repository without naming it.
    ///
    /// It named it literally until the coupling was inverted. Nothing was
    /// leaked by that — a private repository's name is not a secret, and it
    /// 404s to anyone without access — but it made a rename a source edit in a
    /// public repository, and left this repository asserting a name it cannot
    /// verify still exists. The deploy side already hardcodes THIS repository
    /// and derives the release tag from its own clock, so it is the only side
    /// that needs to know the other is there.
    ///
    /// Both halves are the assertion. The variable must be wired, or the
    /// hand-off degrades to prose that names no checkout at all; and the
    /// literal must be absent, or the variable is decoration over a hardcode.
    #[test]
    fn the_ship_handoff_takes_the_deploy_repository_from_configuration() {
        let workflow = ".github/workflows/deploy.yml";
        let text = fs::read_to_string(workspace().join(workflow))
            .unwrap_or_else(|error| panic!("read {workflow}: {error}"));
        assert!(
            text.contains("DEPLOY_REPO: ${{ vars.DEPLOY_REPO }}"),
            "{workflow} must take the deploy repository from an Actions variable"
        );
        assert!(
            text.contains("${DEPLOY_REPO}"),
            "{workflow} sets DEPLOY_REPO but never reads it, so the hand-off names no checkout"
        );
        assert!(
            !text.contains("navigator-deploy"),
            "{workflow} hardcodes the deploy repository; it belongs in the DEPLOY_REPO variable"
        );
    }

    #[test]
    fn instructed_deployments_reads_literals_and_skips_placeholders() {
        let text = "run `navigator ops ship --deployment neon-law-stg --tag 26.1.1`, then \
                    `navigator ops ship --deployment=acme-stg --dry-run`; the general \
                    form is `navigator ops ship --deployment <name>` or --deployment $NAME";
        assert_eq!(
            instructed_deployments(text),
            vec!["neon-law-stg".to_string(), "acme-stg".to_string()]
        );
    }

    /// The CI parity gate. Every requirement that applies to a deployment must
    /// be satisfied by that deployment's own files.
    ///
    /// Deliberately not "every deployment carries every key": requirements
    /// carry an optional `project_id`, and the GitHub webhook five-tuple is
    /// scoped to the automation home alone. Measuring every deployment against
    /// the full list would demand that singleton's receiver credentials
    /// everywhere, which is the opposite of what `store::deployment` says.
    ///
    /// Names only. This reads two files and never decrypts, so it needs no
    /// credential, no KMS permission, and no network.
    #[test]
    fn every_deployment_satisfies_the_requirements_that_apply_to_it() {
        let root = fixture_tree();
        let deployments = names(&root).expect("the deployments tree is readable");
        assert!(
            !deployments.is_empty(),
            "the tree must describe at least one deployment"
        );

        for name in deployments {
            let deployment = Deployment::load(&root, &name).expect("the deployment loads");
            // An unprovisioned row describes cloud resources nobody has created
            // yet, so its coordinates are deliberately blank. `ops ship` refuses
            // one outright; demanding completeness here would only force
            // placeholder values into the tree that read as real.
            if !deployment.provisioned {
                continue;
            }
            let missing = unsatisfied_requirements(&deployment);
            assert!(
                missing.is_empty(),
                "{name} does not satisfy {}: {}. Add each key to \
                 deployments/{name}/{CONFIG_FILE} (a coordinate) or \
                 deployments/{name}/{SECRETS_FILE} (key material).",
                missing.len(),
                missing.iter().map(describe).collect::<Vec<_>>().join(", ")
            );
        }
    }

    /// The guard that a decrypted file can never be committed. `Deployment::load`
    /// refuses a plaintext value, so loading every deployment *is* the check —
    /// but a stray `secrets.yaml` beside the encrypted one would not be loaded
    /// at all, so the tree is walked for anything that looks like key material.
    #[test]
    fn no_plaintext_key_material_sits_in_the_tree() {
        let root = fixture_tree();
        for name in names(&root).expect("the deployments tree is readable") {
            let dir = root.join(TREE).join(&name);
            for entry in fs::read_dir(&dir).expect("a deployment directory is readable") {
                let path = entry.expect("a directory entry").path();
                let file_name = path
                    .file_name()
                    .expect("a file has a name")
                    .to_string_lossy()
                    .into_owned();
                if file_name == CONFIG_FILE {
                    continue;
                }
                assert_eq!(
                    file_name, SECRETS_FILE,
                    "deployments/{name}/{file_name} is neither the plaintext coordinates nor the \
                     encrypted key material. A decrypted file must never be written here — see \
                     .gitignore and docs/deployment-secrets.md."
                );
                encrypted_key_names(&fs::read_to_string(&path).expect("readable"))
                    .unwrap_or_else(|error| panic!("deployments/{name}/{file_name}: {error}"));
            }
        }
    }

    /// `sops` reads its key from `.sops.yaml`, and `config.toml` records the
    /// key the deployment is supposed to use. If they disagree, one
    /// deployment's key material is encrypted against another deployment's
    /// key — which is exactly the isolation this design turns on, failing
    /// silently, because both files still decrypt for whoever holds both keys.
    #[test]
    fn each_deployment_declares_the_kms_key_its_sops_rule_uses() {
        let root = fixture_tree();
        let rules = fs::read_to_string(root.join(".sops.yaml")).expect("read .sops.yaml");
        for name in names(&root).expect("the deployments tree is readable") {
            let deployment = Deployment::load(&root, &name).expect("the deployment loads");
            assert!(
                rules.contains(&deployment.kms_key),
                "no .sops.yaml creation rule names {name}'s key {}",
                deployment.kms_key
            );
            assert!(
                deployment.kms_key.contains(deployment.project_id()),
                "{name}'s key must live in its own project ({}), so staging's key is not \
                 decryptable by production's principals",
                deployment.project_id()
            );
        }
    }

    const ENCRYPTED: &str = r"
SESSION_SECRET: ENC[AES256_GCM,data:aaaa,iv:bbbb,tag:cccc,type:str]
NAVIGATOR_SURREAL_PASSWORD: ENC[AES256_GCM,data:dddd,iv:eeee,tag:ffff,type:str]
sops:
    kms: []
    gcp_kms:
        - resource_id: projects/neon-law-stg/locations/us-west4/keyRings/navigator-secrets/cryptoKeys/deployment-config
    version: 3.13.3
";

    #[test]
    fn key_names_come_back_without_the_sops_metadata_block() {
        let names = encrypted_key_names(ENCRYPTED).expect("a sops document parses");
        assert_eq!(
            names,
            ["NAVIGATOR_SURREAL_PASSWORD", "SESSION_SECRET"]
                .map(String::from)
                .into()
        );
    }

    #[test]
    fn a_plaintext_value_beside_encrypted_ones_is_refused() {
        // The accident this guards: an operator adds a key by hand, forgets
        // `sops --encrypt --in-place`, and commits a live credential. It
        // cannot be un-committed afterwards — only rotated at the provider.
        let document = format!("{ENCRYPTED}\nSENDGRID_API_KEY: SG.a-real-looking-key\n");
        let error = encrypted_key_names(&document).expect_err("plaintext must be refused");
        let rendered = error.to_string();
        assert!(rendered.contains("SENDGRID_API_KEY"));
        assert!(rendered.contains("is not encrypted"));
        assert!(
            !rendered.contains("SG.a-real-looking-key"),
            "the guard must name the key, never echo the value it caught"
        );
    }

    #[test]
    fn an_unencrypted_document_is_refused_even_with_no_values_to_check() {
        let error = encrypted_key_names("SESSION_SECRET: ENC[AES256_GCM,data:a]\n")
            .expect_err("a document with no sops block was never encrypted");
        assert!(error.to_string().contains("never encrypted"));
    }

    #[test]
    fn a_whole_file_envelope_is_refused() {
        // `sops --encrypt` over a non-YAML input produces a single `data`
        // blob. It decrypts identically and destroys the reason for choosing
        // this design: a rotation would be one opaque line.
        let blob = r"
data: ENC[AES256_GCM,data:everything,iv:b,tag:c,type:str]
sops:
    version: 3.13.3
";
        let error = encrypted_key_names(blob).expect_err("a blob must be refused");
        assert!(error.to_string().contains("per value"));
    }

    fn deployment(coordinates: &[(&str, &str)], encrypted: &[&str]) -> Deployment {
        Deployment {
            name: "example".into(),
            kms_key: "projects/example/locations/us-west4/keyRings/k/cryptoKeys/c".into(),
            provisioned: true,
            coordinates: coordinates
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            encrypted_keys: encrypted.iter().map(|k| (*k).to_owned()).collect(),
        }
    }

    /// Every `WEB_REQUIREMENTS` alternative's first spelling, which is what a
    /// complete deployment carries.
    fn complete_keys() -> Vec<&'static str> {
        store::deployment::WEB_REQUIREMENTS
            .iter()
            .filter_map(|requirement| requirement.any_of.first())
            .flat_map(|alternative| alternative.iter().copied())
            .collect()
    }

    #[test]
    fn a_complete_deployment_has_no_unsatisfied_requirement() {
        let complete = complete_keys();
        let deployment = deployment(
            &[(
                PROJECT_ID,
                store::deployment::GITHUB_AUTOMATION_HOME_PROJECT,
            )],
            &complete,
        );
        let missing = unsatisfied_requirements(&deployment);
        assert!(
            missing.is_empty(),
            "unsatisfied: {:?}",
            missing.iter().map(describe).collect::<Vec<_>>()
        );
    }

    /// The shape a signature-free row ships in: a deployment that executes no
    /// documents carries no `DOCUSIGN_*` at all and is still complete.
    ///
    /// This is the level `ops ship` measures at, so it is the level that has
    /// to agree with `portal`'s provider selection. Before `DocuSign` was
    /// trigger-gated, the only way past this preflight was a placeholder — and
    /// a placeholder selects the *real* provider, because
    /// `DocuSignSignatureProvider::from_env` accepts any non-empty value. The
    /// deployment would ship green and fail on its first signature request.
    #[test]
    fn a_deployment_that_signs_nothing_is_complete_without_docusign() {
        let without_docusign: Vec<&str> = complete_keys()
            .into_iter()
            .filter(|key| !key.starts_with("DOCUSIGN_"))
            .collect();
        let deployment = deployment(&[(PROJECT_ID, "neon-law-stg")], &without_docusign);

        let missing = unsatisfied_requirements(&deployment);
        assert!(
            missing.is_empty(),
            "a deployment declaring no DOCUSIGN_BASE_URL must be asked for no DocuSign key: {:?}",
            missing.iter().map(describe).collect::<Vec<_>>()
        );
    }

    /// The paired half at this level: declaring the integration and then
    /// half-supplying it is still a hard failure, so the gate cannot be
    /// escaped by naming the base URL alone.
    #[test]
    fn declaring_docusign_and_omitting_its_auth_is_still_unsatisfied() {
        // `DOCUSIGN_BASE_URL` is the declaration, not a requirement, so it is
        // absent from `complete_keys()` and has to be added deliberately —
        // which is exactly the operator action this test describes.
        let mut base_url_only: Vec<&str> = complete_keys()
            .into_iter()
            .filter(|key| !key.starts_with("DOCUSIGN_"))
            .collect();
        base_url_only.push("DOCUSIGN_BASE_URL");
        let deployment = deployment(&[(PROJECT_ID, "neon-law-stg")], &base_url_only);

        let missing = unsatisfied_requirements(&deployment);
        assert!(
            !missing.is_empty(),
            "DOCUSIGN_BASE_URL alone must not satisfy the DocuSign requirements"
        );
    }

    #[test]
    fn a_missing_required_key_is_reported() {
        // The drift this whole tree exists to make mechanical.
        let complete: Vec<&str> = complete_keys()
            .into_iter()
            .filter(|key| *key != "SENDGRID_API_KEY")
            .collect();
        let deployment = deployment(
            &[(
                PROJECT_ID,
                store::deployment::GITHUB_AUTOMATION_HOME_PROJECT,
            )],
            &complete,
        );
        let missing = unsatisfied_requirements(&deployment);
        assert_eq!(
            missing.iter().map(describe).collect::<Vec<_>>(),
            vec!["SENDGRID_API_KEY"]
        );
    }

    #[test]
    fn the_webhook_five_tuple_is_required_of_the_automation_home_alone() {
        // The subtlety a "every deployment has every key" test gets wrong.
        // `store::deployment` scopes the engineering webhook receiver to one
        // project, so its absence is a gap there and correct everywhere else.
        let without_webhook: Vec<&str> = complete_keys()
            .into_iter()
            .filter(|key| !key.starts_with("NAVIGATOR_GITHUB_WEBHOOK"))
            .collect();

        let home = deployment(
            &[(
                PROJECT_ID,
                store::deployment::GITHUB_AUTOMATION_HOME_PROJECT,
            )],
            &without_webhook,
        );
        assert!(
            !unsatisfied_requirements(&home).is_empty(),
            "the automation home must still need its receiver credentials"
        );

        let elsewhere = deployment(&[(PROJECT_ID, "neon-law")], &without_webhook);
        assert!(
            unsatisfied_requirements(&elsewhere).is_empty(),
            "no other deployment may carry that singleton's receiver credentials"
        );
    }

    #[test]
    fn inline_env_keys_count_as_supplied_without_entering_the_encrypted_file() {
        // `NAVIGATOR_STORAGE_BACKEND` and friends are boot-required but ship
        // as inline Deployment env. Requiring them in the tree would push
        // four non-secrets into an encrypted file for no reason, so a
        // deployment that omits them entirely must still satisfy every
        // requirement.
        let without_inline: Vec<&str> = complete_keys()
            .into_iter()
            .filter(|key| !ship::INLINE_ENV_WEB_KEYS.contains(key))
            .collect();
        let deployment = deployment(&[(PROJECT_ID, "neon-law")], &without_inline);

        for key in ship::INLINE_ENV_WEB_KEYS {
            assert!(!deployment.encrypted_keys.contains(*key));
            assert!(deployment.supplied_keys().contains(*key));
        }
        assert!(unsatisfied_requirements(&deployment).is_empty());
    }

    #[test]
    fn the_plan_names_every_projected_object_that_nothing_supplies() {
        let deployment = deployment(&[(PROJECT_ID, "neon-law")], &[]);
        let error = plan(&deployment).expect_err("an empty deployment supplies nothing");
        let rendered = error.to_string();
        assert!(rendered.contains("SESSION_SECRET"));
        assert!(rendered.contains("Secret Manager was not changed"));
    }

    #[test]
    fn a_production_row_is_not_asked_for_the_automation_home_s_webhook_secret() {
        // The `SecretProviderClass` is one manifest rendered for every
        // deployment, so it references the engineering webhook receiver's
        // credentials everywhere. Demanding them would push a singleton's HMAC
        // secret into a row holding real client matters — the opposite of what
        // `store::deployment` scopes it to.
        let objects: Vec<String> = projected_objects().into_iter().collect();
        let home_only = [
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "NAVIGATOR_GITHUB_APP_LOGIN",
        ];
        let supplied: Vec<&str> = objects
            .iter()
            .map(String::as_str)
            .filter(|object| !home_only.contains(object))
            .collect();

        let (plan, skipped) = plan(&deployment(&[(PROJECT_ID, "neon-law")], &supplied))
            .expect("a production row without the receiver's credentials is complete");
        for key in home_only {
            assert!(!plan.contains_key(key));
            assert_eq!(
                skipped.get(key),
                Some(&Exemption::ScopedElsewhere),
                "{key} must be reported as skipped, never silently dropped"
            );
        }
    }

    /// The JWT triple every deployment signs into `DOCUSIGN_BASE_URL` with
    /// today, and which the `SecretProviderClass` therefore references.
    const DOCUSIGN_JWT_TRIPLE: [&str; 3] = [
        "DOCUSIGN_INTEGRATION_KEY",
        "DOCUSIGN_USER_ID",
        "DOCUSIGN_PRIVATE_KEY",
    ];

    #[test]
    fn an_alternative_the_deployment_satisfies_another_way_is_not_demanded() {
        // `WEB_REQUIREMENTS` models DocuSign auth as the JWT triple OR an
        // access token, and the manifest references only the triple. A
        // deployment that authenticates with a token instead must not be
        // asked for the three keys it correctly does not have.
        let objects: Vec<String> = projected_objects().into_iter().collect();
        let mut token_only: Vec<&str> = objects
            .iter()
            .map(String::as_str)
            .filter(|object| !DOCUSIGN_JWT_TRIPLE.contains(object))
            .collect();
        token_only.push("DOCUSIGN_ACCESS_TOKEN");

        let (plan, skipped) = plan(&deployment(&[(PROJECT_ID, "neon-law")], &token_only))
            .expect("an access token satisfies DocuSign auth");
        for key in DOCUSIGN_JWT_TRIPLE {
            assert!(!plan.contains_key(key));
            assert_eq!(
                skipped.get(key),
                Some(&Exemption::AlternativeSatisfied),
                "{key} must be reported as skipped, never silently dropped"
            );
        }
    }

    #[test]
    fn dropping_the_whole_docusign_alternative_set_is_still_missing() {
        // The other half: the exemption is "satisfied another way", not
        // "optional". Supply neither alternative and it is a real gap again.
        let objects: Vec<String> = projected_objects().into_iter().collect();
        let neither: Vec<&str> = objects
            .iter()
            .map(String::as_str)
            .filter(|object| !DOCUSIGN_JWT_TRIPLE.contains(object))
            .collect();

        let error = plan(&deployment(&[(PROJECT_ID, "neon-law")], &neither))
            .expect_err("no DocuSign auth at all is a gap");
        assert!(error.to_string().contains("DOCUSIGN_INTEGRATION_KEY"));
    }

    #[test]
    fn the_automation_home_supplies_every_object_the_manifest_references() {
        // `neon-law-stg` is the automation home and declares every integration
        // the shared object list names, so it is the one deployment that skips
        // nothing. It is therefore the canary for a genuinely untrimmed
        // manifest: an object no deployment supplies shows up here as a skip
        // rather than hiding behind another row's exemption.
        // `DOCUSIGN_ACCESS_TOKEN` sat in the manifest exactly that way.
        //
        // Rows that legitimately skip — one that is not the automation home,
        // or one that declares no DocuSign — are covered by `ship`, which
        // renders the object list per deployment so the class references only
        // what that deployment writes. Filtering to the automation home is
        // what this test always meant: it looped over every provisioned row
        // while the tree had none, so demanding zero skips of a row entitled
        // to skip was a latent contradiction nothing could reach.
        let root = fixture_tree();
        for deployment in provisioned_deployments(&root)
            .into_iter()
            .filter(|deployment| deployment.project_id() == GITHUB_AUTOMATION_HOME_PROJECT)
        {
            let name = &deployment.name;
            let (_, skipped) = plan(&deployment).expect("the tree supplies every projected object");
            assert!(
                skipped.is_empty(),
                "the SecretProviderClass references {} object(s) {name} will not write: {:?}. \
                 Trim each from examples/deploy/k8s/gke/secrets/secret-provider-class.yaml, or \
                 supply a value.",
                skipped.len(),
                skipped.keys().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_deployment_that_declines_docusign_skips_every_docusign_object() {
        // The failure this whole seam exists for: a row that supplies no
        // `DOCUSIGN_BASE_URL`, so it declines the integration and runs
        // `StubSignatureProvider`. Every DocuSign object the shared manifest
        // references must read as a skip rather than a missing credential —
        // otherwise the only way to ship is a placeholder, which boots the real
        // provider instead of the stub.
        let root = fixture_tree();
        for deployment in provisioned_deployments(&root)
            .into_iter()
            .filter(|deployment| !deployment.coordinates.contains_key("DOCUSIGN_BASE_URL"))
        {
            let (plan, skipped) = plan(&deployment).expect("a declined integration is not a gap");
            for object in projected_objects()
                .into_iter()
                .filter(|object| object.starts_with("DOCUSIGN_"))
            {
                assert!(!plan.contains_key(&object), "{object} must not be written");
                assert_eq!(
                    skipped.get(&object),
                    Some(&Exemption::Untriggered),
                    "{object} belongs to an integration this deployment declares nothing of"
                );
            }
        }
    }

    #[test]
    fn declaring_docusign_still_demands_every_key_the_provider_reads() {
        // The other half: `Untriggered` is "declined", never "optional". A
        // deployment that sets the trigger and stops there is half-configured,
        // which is the case the trigger was introduced to keep failing.
        let objects: Vec<String> = projected_objects().into_iter().collect();
        let base_url_only: Vec<&str> = objects
            .iter()
            .map(String::as_str)
            .filter(|object| !object.starts_with("DOCUSIGN_") || *object == "DOCUSIGN_BASE_URL")
            .collect();

        let error = plan(&deployment(&[(PROJECT_ID, "neon-law")], &base_url_only))
            .expect_err("declaring DocuSign and supplying nothing else is a gap");
        for key in [
            "DOCUSIGN_ACCOUNT_ID",
            "DOCUSIGN_HMAC_KEY",
            "DOCUSIGN_OAUTH_BASE",
            "DOCUSIGN_SIGNER_EMAIL",
        ] {
            assert!(error.to_string().contains(key), "{key} must still be named");
        }
    }

    #[test]
    fn the_automation_home_is_still_asked_for_its_own_webhook_secret() {
        // The other half: the exemption is scoped to the project that does not
        // own the receiver, never a blanket allowance.
        let objects: Vec<String> = projected_objects().into_iter().collect();
        let supplied: Vec<&str> = objects
            .iter()
            .map(String::as_str)
            .filter(|object| *object != "NAVIGATOR_GITHUB_WEBHOOK_SECRET")
            .collect();

        let error = plan(&deployment(
            &[(
                PROJECT_ID,
                store::deployment::GITHUB_AUTOMATION_HOME_PROJECT,
            )],
            &supplied,
        ))
        .expect_err("the automation home owns the receiver");
        assert!(error
            .to_string()
            .contains("NAVIGATOR_GITHUB_WEBHOOK_SECRET"));
    }

    #[test]
    fn an_unscoped_object_is_never_exempt() {
        let nothing = BTreeSet::new();
        // `SESSION_SECRET` is required of every deployment, so no project may
        // skip it however the rendering manifest is shaped.
        assert_eq!(exemption("SESSION_SECRET", "neon-law", &nothing), None);
        // …and neither is an object no requirement mentions at all: a Drive
        // folder id is projected everywhere on purpose, so its absence is a
        // real gap rather than a scoping decision.
        assert_eq!(
            exemption(
                "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID",
                "neon-law",
                &nothing
            ),
            None
        );
    }

    #[test]
    fn a_coordinate_supplies_a_projected_object_the_encrypted_file_does_not() {
        // Not everything the SecretProviderClass projects is a secret. The
        // Drive folder ids and `NAVIGATOR_FORGE_BACKEND` are coordinates that
        // ride the Secret rail because the pod reads one Secret, not two
        // sources — so the plan must accept them from `config.toml`.
        let objects: Vec<String> = projected_objects().into_iter().collect();
        let encrypted: Vec<&str> = objects
            .iter()
            .filter(|object| *object != "NAVIGATOR_FORGE_BACKEND")
            .map(String::as_str)
            .collect();
        let deployment = deployment(
            &[
                (PROJECT_ID, "neon-law"),
                ("NAVIGATOR_FORGE_BACKEND", "github"),
            ],
            &encrypted,
        );

        let (plan, skipped) = plan(&deployment).expect("a coordinate satisfies the object");
        assert_eq!(plan["NAVIGATOR_FORGE_BACKEND"], Source::Coordinate);
        assert_eq!(plan["SESSION_SECRET"], Source::Encrypted);
        assert!(skipped.is_empty());
    }

    // ---------- locating the tree ----------

    /// A checkout that carries the tree and nothing else: no `Cargo.toml`, no
    /// `k8s/`, so `orchestrate::workspace_root` cannot find it.
    fn deploy_only_checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("create a temp checkout");
        fs::create_dir_all(dir.path().join(TREE).join("neon-law-stg")).expect("create the tree");
        dir
    }

    #[test]
    fn an_explicit_directory_locates_a_tree_the_workspace_walk_cannot_find() {
        // The blocker the flag exists for. The tree does not have to live
        // beside the source that reads it, and the private deploy checkout it
        // will live in has neither of the two markers the walk looks for.
        let checkout = deploy_only_checkout();
        assert_eq!(
            root(Some(checkout.path())).expect("the explicit directory holds the tree"),
            checkout.path()
        );
        assert_eq!(
            names(checkout.path()).expect("the tree is readable"),
            vec!["neon-law-stg".to_owned()]
        );
    }

    #[test]
    fn being_handed_the_tree_itself_says_so_and_names_the_fix() {
        // The reading the flag's name invites. Left alone it produces a
        // `no deployment <name> in deployments/` error describing the wrong
        // problem, and the fix — pass the parent — is not derivable from it.
        let checkout = deploy_only_checkout();
        let error = root(Some(&checkout.path().join(TREE)))
            .expect_err("the tree itself is not the directory containing it");
        let message = error.to_string();
        assert!(
            message.contains("the directory CONTAINING the tree"),
            "the error must say what it wanted: {message}"
        );
        assert!(
            message.contains(&checkout.path().display().to_string()),
            "the error must name the directory to pass instead: {message}"
        );
    }

    #[test]
    fn a_directory_with_no_tree_in_it_is_refused_at_the_flag() {
        let empty = tempfile::tempdir().expect("create a temp directory");
        let error = root(Some(empty.path())).expect_err("there is no tree here");
        assert!(
            error.to_string().contains("no `deployments/` directory"),
            "{error}"
        );
    }

    #[test]
    fn the_workspace_says_where_the_tree_went() {
        // Running `ops ship` from a checkout of THIS repository is the mistake
        // the split creates, and it will be made — the command lived here for
        // as long as the tree did. So the fallback has to say that a private
        // repository holds the tree and give both flags that reach one, because
        // neither is guessable from `no deployment <name> in deployments/`.
        //
        // It does NOT name that repository. Everyone who can roll a cluster
        // already has the checkout, so the name saved them nothing an operator
        // could not supply from memory — and it put a private repository's name
        // in a public error string, where a rename would strand it.
        //
        // This asserts the absence as much as the message: a `deployments/`
        // directory reappearing in the workspace means the tree came back to
        // the public repository, and that is a decision, not a merge artifact.
        let error = root(None).expect_err("this repository holds no deployment tree");
        let message = error.to_string();
        assert!(
            message.contains("private repository"),
            "the error must say a private repository holds the tree: {message}"
        );
        assert!(
            !message.contains("navigator-deploy"),
            "the error names the deploy repository; this repository does not name it: {message}"
        );
        assert!(
            message.contains("--deployments-dir") && message.contains(ROOT_ENV),
            "the error must name both ways to point at one: {message}"
        );
    }

    /// The fixture is reached the same way an operator reaches the real tree:
    /// through `root`, with an explicit directory. Nothing about the resolver
    /// is special-cased for it.
    #[test]
    fn the_fixture_tree_resolves_through_the_ordinary_flag() {
        let root = root(Some(&fixture_tree())).expect("the fixture holds a tree");
        assert_eq!(
            names(&root).expect("the tree is readable"),
            vec![
                "example-automation-home".to_owned(),
                "example-deployment".to_owned()
            ]
        );
    }
}
