use std::{io::ErrorKind, path::Path, process::Command};

use serde::Deserialize;
use serde_yaml::Value;

const KIND: &str = "k8s/overlays/kind";
const KIND_DEPS: &str = "k8s/overlays/kind-deps";
const GKE: &str = "examples/deploy/k8s/gke";

const CLI_README: &str = "cli/README.md";

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has a workspace parent")
}

fn render(path: &str) -> Option<Vec<Value>> {
    let output = match Command::new("kubectl")
        .args(["kustomize", path])
        .current_dir(workspace())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            eprintln!("kubectl unavailable; skipping staging manifest contract test");
            return None;
        }
        Err(error) => panic!("running kubectl kustomize {path}: {error}"),
    };
    assert!(
        output.status.success(),
        "kubectl kustomize {path} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(
        serde_yaml::Deserializer::from_slice(&output.stdout)
            .map(|document| Value::deserialize(document).expect("rendered resource is valid YAML"))
            .collect(),
    )
}

fn rendered_text(resources: &[Value]) -> String {
    serde_yaml::to_string(resources).expect("resources serialize")
}

fn deployment_names(resources: &[Value]) -> Vec<&str> {
    resources
        .iter()
        .filter(|resource| resource["kind"].as_str() == Some("Deployment"))
        .filter_map(|resource| resource["metadata"]["name"].as_str())
        .collect()
}

fn resource<'a>(resources: &'a [Value], kind: &str, name: &str) -> &'a Value {
    resources
        .iter()
        .find(|resource| {
            resource["kind"].as_str() == Some(kind)
                && resource["metadata"]["name"].as_str() == Some(name)
        })
        .unwrap_or_else(|| panic!("{kind}/{name} must be rendered"))
}

/// The store endpoint both workloads must resolve to the same `ConfigMap`.
///
/// `web` and the worker journal to one database: a Notation `web` commits
/// has to be visible to the worker that advances it, so a manifest that
/// points them at different coordinates is a `RecordNotFound` on every
/// Restate-backed flow rather than a config nit.
fn store_endpoint(resource: &Value) -> &str {
    resource["spec"]["template"]["spec"]["containers"]
        .as_sequence()
        .expect("workload containers")
        .iter()
        .flat_map(|container| container["env"].as_sequence().into_iter().flatten())
        .find(|entry| entry["name"].as_str() == Some("NAVIGATOR_SURREAL_ENDPOINT"))
        .and_then(|entry| entry["valueFrom"]["configMapKeyRef"]["name"].as_str())
        .expect("NAVIGATOR_SURREAL_ENDPOINT must be ConfigMap-backed")
}

fn env_entry<'a>(resource: &'a Value, name: &str) -> &'a Value {
    resource["spec"]["template"]["spec"]["containers"]
        .as_sequence()
        .expect("workload containers")
        .iter()
        .flat_map(|container| container["env"].as_sequence().into_iter().flatten())
        .find(|entry| entry["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("{name} must be configured"))
}

fn config_map_key<'a>(resource: &'a Value, env_name: &str) -> &'a str {
    let matches = resource["spec"]["template"]["spec"]["containers"]
        .as_sequence()
        .expect("workload containers")
        .iter()
        .flat_map(|container| container["env"].as_sequence().into_iter().flatten())
        .filter(|entry| entry["name"].as_str() == Some(env_name))
        .filter_map(|entry| entry["valueFrom"]["configMapKeyRef"]["key"].as_str())
        .collect::<Vec<_>>();
    // Kubernetes resolves duplicate env names to the last entry, so accepting the
    // first match would let a stray duplicate ship the wrong bucket while the pod
    // boots with a different value. Require the coordinate to be defined exactly once.
    match matches.as_slice() {
        [key] => key,
        [] => panic!("{env_name} must be ConfigMap-backed"),
        _ => panic!("{env_name} must be defined exactly once"),
    }
}

#[test]
fn staging_contracts_keep_secrets_lanes_brand_and_store_coordinates_aligned() {
    for overlay in [KIND] {
        let Some(resources) = render(overlay) else {
            return;
        };
        let text = rendered_text(&resources);
        let web = resources
            .iter()
            .find(|resource| {
                resource["kind"].as_str() == Some("Deployment")
                    && resource["metadata"]["name"]
                        .as_str()
                        .is_some_and(|name| name.ends_with("navigator-web"))
            })
            .expect("staging web deployment");
        let worker = resources
            .iter()
            .find(|resource| resource["kind"].as_str() == Some("RestateDeployment"))
            .expect("staging worker");
        assert_eq!(store_endpoint(web), store_endpoint(worker));
        assert_eq!(
            config_map_key(worker, "NAVIGATOR_DOCUMENTS_BUCKET"),
            "documents_bucket"
        );
        assert_eq!(
            config_map_key(worker, "NAVIGATOR_STORAGE_BUCKET"),
            "exports_bucket"
        );
        assert!(text.contains("NAVIGATOR_CUSTOM_BRANDING"));
        assert!(!text.contains("NAVIGATOR_BRAND_BUNDLE_DIR"));
        assert_eq!(
            env_entry(web, "NAVIGATOR_BOOTSTRAP_COMPANY")["value"].as_str(),
            Some(store::seed::FIRM_ENTITY_NAME),
            "{overlay} must protect the firm anchor entity — the legal person, \
             not the `Neon Law` mark it trades under"
        );
        // The application environment value is `dev` for the cloud staging
        // lane: it shares the one development profile with local KIND.
        // "staging" survives only as the deployment-lane label asserted
        // below, never as a `NAVIGATOR_ENVIRONMENT` value.
        assert_eq!(
            env_entry(web, "NAVIGATOR_ENVIRONMENT")["value"].as_str(),
            Some("dev"),
            "{overlay} web must emit NAVIGATOR_ENVIRONMENT=dev"
        );
        assert_eq!(
            env_entry(worker, "NAVIGATOR_ENVIRONMENT")["value"].as_str(),
            Some("dev"),
            "{overlay} worker must emit NAVIGATOR_ENVIRONMENT=dev"
        );
        assert!(text.contains("readOnly: true"));
        for lane in ["documents", "assets", "exports", "lfs"] {
            assert!(text.contains(lane), "{overlay} must preserve {lane} lane");
        }
        assert!(text.contains("navigator.neonlaw.org/environment: staging"));
        assert!(
            !rendered_text(std::slice::from_ref(web)).contains("restate.restate.svc.cluster.local")
        );
    }
    let Some(kind_deps) = render(KIND_DEPS) else {
        return;
    };
    assert!(kind_deps
        .iter()
        .all(|resource| { resource["metadata"]["name"].as_str() != Some("navigator-web") }));
}

#[test]
fn staging_operator_docs_match_the_implemented_dev_lifecycle() {
    let cli_readme = std::fs::read_to_string(workspace().join(CLI_README)).expect("CLI README");
    assert!(
        cli_readme.contains("NAVIGATOR_ENVIRONMENT=dev"),
        "the local staging command must require the dev application profile"
    );
    assert!(
        !cli_readme.contains("NAVIGATOR_ENVIRONMENT=staging"),
        "staging is a lifecycle target, not an application environment"
    );
}

#[test]
fn kind_identity_provider_is_rauthy() {
    for overlay in [KIND, KIND_DEPS] {
        let Some(resources) = render(overlay) else {
            return;
        };
        let deployments = deployment_names(&resources);
        assert!(
            deployments.contains(&"rauthy"),
            "{overlay} must deploy Rauthy: {deployments:?}"
        );
        assert!(
            !deployments.contains(&"keycloak"),
            "{overlay} must not retain the retired identity provider: {deployments:?}"
        );
    }
}

#[test]
fn kind_rauthy_bootstrap_matches_the_local_login_contract() {
    let Some(resources) = render(KIND) else {
        return;
    };
    let bootstrap = resource(&resources, "ConfigMap", "rauthy-bootstrap");
    let users: serde_json::Value = serde_json::from_str(
        bootstrap["data"]["users.json"]
            .as_str()
            .expect("Rauthy users.json"),
    )
    .expect("valid users.json");
    let users = users.as_array().expect("Rauthy users are an array");
    for (email, username, password) in [
        ("owner@neonlaw.com", "owner", "password"),
        ("admin@neonlaw.com", "admin", "password"),
        ("lawyer@neonlaw.com", "lawyer", "password"),
        ("clerk@neonlaw.com", "clerk", "password"),
        ("client@neonlaw.com", "client", "password"),
    ] {
        let user = users
            .iter()
            .find(|user| user["email"].as_str() == Some(email))
            .unwrap_or_else(|| panic!("Rauthy must seed {email}"));
        assert_eq!(user["preferred_username"].as_str(), Some(username));
        assert_eq!(user["password"]["Plain"].as_str(), Some(password));
        assert_eq!(user["email_verified"].as_bool(), Some(true));
    }

    let clients: serde_json::Value = serde_json::from_str(
        bootstrap["data"]["clients.json"]
            .as_str()
            .expect("Rauthy clients.json"),
    )
    .expect("valid clients.json");
    let client = clients
        .as_array()
        .expect("Rauthy clients are an array")
        .iter()
        .find(|client| client["id"].as_str() == Some("navigator-web"))
        .expect("navigator-web client");
    assert_eq!(
        client["secret"]["Plain"]
            .as_str()
            .expect("confidential client secret")
            .len(),
        64,
        "Rauthy requires a 64-character confidential-client secret"
    );
    assert_eq!(
        client["redirect_uris"],
        serde_json::json!(["http://localhost:*"])
    );
    assert!(
        client.get("allowed_origins").is_none(),
        "Rauthy permits loopback wildcards for redirects, not CORS origins"
    );
    assert_eq!(client["challenges"], serde_json::json!(["S256"]));
    assert_eq!(client["id_token_alg"].as_str(), Some("RS256"));
    assert_eq!(client["force_mfa"].as_bool(), Some(false));

    let config = bootstrap["data"]["config.toml"]
        .as_str()
        .expect("Rauthy config.toml");
    assert!(config.contains("insecure_cookie = true"));
    assert!(config.contains("enable = false"));

    let secrets = resource(&resources, "Secret", "rauthy-secrets");
    assert_eq!(
        secrets["stringData"]["BOOTSTRAP_ADMIN_EMAIL"].as_str(),
        Some("nick@neonlaw.com")
    );
    assert_eq!(
        secrets["stringData"]["BOOTSTRAP_ADMIN_PASSWORD_PLAIN"].as_str(),
        Some("admin")
    );
    assert_eq!(
        secrets["stringData"]["OAUTH_ISSUER_URL"].as_str(),
        Some("http://localhost:30080/auth/v1/"),
        "the application issuer must exactly match Rauthy's discovery metadata"
    );
    let encryption_key = secrets["stringData"]["ENC_KEYS"]
        .as_str()
        .expect("Rauthy encryption key");
    let (key_id, _) = encryption_key
        .split_once('/')
        .expect("Rauthy encryption key embeds its ID");
    assert_eq!(
        key_id.len(),
        6,
        "Rauthy encryption-key IDs are exactly six characters"
    );
    assert_eq!(
        secrets["stringData"]["ENC_KEY_ACTIVE"].as_str(),
        Some(key_id),
        "the active Rauthy encryption-key ID must identify the fixture key"
    );
}

#[test]
fn reusable_rauthy_layer_contains_no_kind_credentials() {
    let staging =
        std::fs::read_to_string(workspace().join("k8s/staging/rauthy.yaml")).expect("Rauthy");
    for local_only in [
        "\"Plain\": \"password\"",
        "BOOTSTRAP_ADMIN_PASSWORD_PLAIN: admin",
        "http://localhost:*",
        "navigatorwebsecret",
    ] {
        assert!(
            !staging.contains(local_only),
            "reusable Rauthy layer must not contain `{local_only}`"
        );
    }
    for environment_owned in ["rauthy-secrets", "rauthy-client", "rauthy-bootstrap"] {
        assert!(
            staging.contains(environment_owned),
            "reusable Rauthy layer must require {environment_owned}"
        );
    }
}

#[test]
fn production_overlay_excludes_the_local_rauthy_fixture() {
    let Some(resources) = render(GKE) else {
        return;
    };
    let production = rendered_text(&resources);
    for local_only in [
        "rauthy-bootstrap",
        "users.json",
        "\"Plain\": \"password\"",
        "BOOTSTRAP_ADMIN_PASSWORD_PLAIN",
    ] {
        assert!(
            !production.contains(local_only),
            "production must exclude the local Rauthy fixture value `{local_only}`"
        );
    }
}

/// Once a real `RESTATE_IDENTITY_KEY` is configured, the worker's Restate SDK
/// endpoint requires a valid Restate Cloud signature on every request it
/// serves, including its own `/restate/health`. Neither kubelet's readiness
/// probe nor the GCE LB's `BackendConfig` health check ever carries that
/// signature, so a health check proxied through to the worker 401s forever.
/// Envoy must answer this specific path itself instead of forwarding it.
#[test]
fn workflows_service_envoy_answers_the_health_probe_without_proxying_to_the_worker() {
    let Some(resources) = render(GKE) else {
        return;
    };
    let config_map = resource(&resources, "ConfigMap", "workflows-service-envoy");
    let envoy_yaml = config_map["data"]["envoy.yaml"]
        .as_str()
        .expect("envoy.yaml embedded config");
    let envoy_config: Value = serde_yaml::from_str(envoy_yaml).expect("valid envoy config YAML");
    let routes = envoy_config["static_resources"]["listeners"][0]["filter_chains"][0]["filters"][0]
        ["typed_config"]["route_config"]["virtual_hosts"][0]["routes"]
        .as_sequence()
        .expect("configured routes");
    let health_route = routes
        .iter()
        .find(|route| route["match"]["path"].as_str() == Some("/restate/health"))
        .expect("a dedicated /restate/health route");
    assert!(
        health_route["direct_response"]["status"].as_u64() == Some(200),
        "envoy must answer /restate/health itself with a direct_response, not a proxy"
    );
    assert_ne!(
        health_route["route"]["cluster"].as_str(),
        Some("restate_worker"),
        "the health check must never reach the identity-gated worker endpoint"
    );
}
