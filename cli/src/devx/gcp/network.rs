//! Provision the Neon Law Navigator VPC.
//!
//! ## Scope
//!
//! Each deployment gets one custom-mode VPC and one explicitly named regional
//! subnet with Private Google Access. GKE Autopilot selects both names. Every
//! managed service the workloads reach is a public Google endpoint, so
//! private-services-access peering is not required.
//!
//! Should a private-IP managed service ever arrive, the additions go here:
//! subnet → global address (`PURPOSE=VPC_PEERING`) →
//! `servicenetworking.connections.create`. All three follow the
//! same insert-then-poll-LRO pattern the other steps use.
//!
//! ## Idempotency
//!
//! `compute.networks.insert` returns HTTP **409 Conflict** when a
//! network with the same name already exists — same trick as
//! buckets. The LRO poll is skipped on 409.
//! A newly enabled Compute API can briefly return `SERVICE_DISABLED`
//! after the Service Usage operation completes. The first VPC insert
//! retries only that exact propagation response; unrelated 403s still
//! fail immediately.

use std::time::Duration;

use serde_json::json;

use super::client::{GcpClient, GcpService, Mode};
use super::error::{SetupError, SetupResult};
use super::{lro, services, SetupConfig};

/// Default VPC network name. Overridable via `NAVIGATOR_VPC_NAME`.
pub const DEFAULT_NETWORK_NAME: &str = "navigator-vpc";
/// Default regional subnetwork name. Overridable via
/// `NAVIGATOR_SUBNETWORK_NAME`.
pub const DEFAULT_SUBNETWORK_NAME: &str = "navigator-subnet";
/// Compute's service activation can lag the completed Service Usage LRO.
const API_ACTIVATION_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Bound activation propagation retries to two minutes.
const API_ACTIVATION_MAX_ATTEMPTS: usize = 25;

pub async fn ensure_network(
    client: &GcpClient,
    project_id: &str,
    config: &SetupConfig,
) -> SetupResult<()> {
    ensure_named_network(client, project_id, &config.vpc_name).await
}

/// Ensure the regional Cloud Router and Cloud NAT that give the deployment's
/// private GKE nodes outbound connectivity.
///
/// The cluster name is the deployment name throughout `ops gcp setup`, so the
/// two resources remain distinct when multiple deployments share a project.
pub async fn ensure_router_and_nat(
    client: &GcpClient,
    project_id: &str,
    config: &SetupConfig,
) -> SetupResult<()> {
    ensure_named_router_and_nat(
        client,
        project_id,
        &config.region,
        &config.vpc_name,
        &config.cluster_name,
    )
    .await
}

fn router_name(deployment_name: &str) -> String {
    format!("{deployment_name}-router")
}

fn nat_name(deployment_name: &str) -> String {
    format!("{deployment_name}-nat")
}

fn router_path(project_id: &str, region: &str, router_name: &str) -> String {
    format!("/compute/v1/projects/{project_id}/regions/{region}/routers/{router_name}")
}

/// The one NAT gateway this deployment owns.
fn nat_body(nat_name: &str) -> serde_json::Value {
    json!({
        "name": nat_name,
        "natIpAllocateOption": "AUTO_ONLY",
        "sourceSubnetworkIpRangesToNat": "ALL_SUBNETWORKS_ALL_IP_RANGES",
    })
}

fn router_body(
    project_id: &str,
    network_name: &str,
    router_name: &str,
    nat_name: &str,
) -> serde_json::Value {
    json!({
        "name": router_name,
        "network": format!("projects/{project_id}/global/networks/{network_name}"),
        "bgp": { "asn": 64514 },
        "nats": [nat_body(nat_name)],
    })
}

/// A Compute resource reference stripped of the API host and version a live
/// response carries.
///
/// `routers.get` answers with a full selfLink
/// (`https://www.googleapis.com/compute/v1/projects/…`) while the create body
/// carries the relative `projects/…` form. Comparing the two verbatim never
/// matches, which would report every converged router as drifted.
fn relative_resource(reference: &str) -> &str {
    reference
        .rsplit_once("/compute/v1/")
        .map_or(reference, |(_, tail)| tail)
}

/// Whether a live router sits on the VPC this deployment is provisioning.
fn network_matches(router: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match (
        router.get("network").and_then(serde_json::Value::as_str),
        expected.get("network").and_then(serde_json::Value::as_str),
    ) {
        (Some(live), Some(wanted)) => relative_resource(live) == relative_resource(wanted),
        _ => false,
    }
}

/// The NAT gateways a live router currently carries.
fn live_nats(router: &serde_json::Value) -> &[serde_json::Value] {
    router
        .get("nats")
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Whether the router already carries this deployment's gateway with the
/// egress settings private nodes need.
fn has_expected_nat(router: &serde_json::Value, nat_name: &str) -> bool {
    live_nats(router).iter().any(|nat| {
        nat.get("name").and_then(serde_json::Value::as_str) == Some(nat_name)
            && nat
                .get("natIpAllocateOption")
                .and_then(serde_json::Value::as_str)
                == Some("AUTO_ONLY")
            && nat
                .get("sourceSubnetworkIpRangesToNat")
                .and_then(serde_json::Value::as_str)
                == Some("ALL_SUBNETWORKS_ALL_IP_RANGES")
    })
}

fn router_has_expected_nat(
    router: &serde_json::Value,
    expected: &serde_json::Value,
    nat_name: &str,
) -> bool {
    network_matches(router, expected) && has_expected_nat(router, nat_name)
}

/// The `nats` list a drift patch is entitled to submit.
///
/// `compute.routers.patch` replaces a repeated field wholesale rather than
/// merging into it, so submitting this deployment's single gateway would
/// delete every other NAT on a router the project shares. Carry the live
/// entries through untouched and rewrite only the gateway this deployment owns
/// by name, so convergence can never remove topology setup did not create.
fn merged_nats(existing: &serde_json::Value, nat_name: &str) -> Vec<serde_json::Value> {
    let mut nats: Vec<serde_json::Value> = live_nats(existing)
        .iter()
        .filter(|nat| nat.get("name").and_then(serde_json::Value::as_str) != Some(nat_name))
        .cloned()
        .collect();
    nats.push(nat_body(nat_name));
    nats
}

/// The drift patch itself: this deployment's gateway alongside every unrelated
/// one, and nothing else.
///
/// `bgp` is deliberately absent. The create seeds a placeholder ASN because a
/// router needs one, and replaying it here would rewrite the ASN of a router
/// carrying real BGP sessions.
fn nat_patch_body(
    existing: &serde_json::Value,
    router_name: &str,
    nat_name: &str,
) -> serde_json::Value {
    json!({
        "name": router_name,
        "nats": merged_nats(existing, nat_name),
    })
}

/// Ensure the named Cloud Router and its NAT configuration.
///
/// The normal path reads first: a matching router is a no-op and an existing
/// router missing this deployment's gateway receives a patch that adds it
/// without disturbing any other gateway on the same router. A router of this
/// name on a different VPC is somebody else's resource, not drift, so setup
/// stops rather than moving it. An absent router is created and its Compute
/// LRO is polled. A conflict on that create is another actor having won the
/// same idempotent race, so it succeeds without a second poll.
async fn ensure_named_router_and_nat(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    network_name: &str,
    deployment_name: &str,
) -> SetupResult<()> {
    let router_name = router_name(deployment_name);
    let nat_name = nat_name(deployment_name);
    let path = router_path(project_id, region, &router_name);
    let body = router_body(project_id, network_name, &router_name, &nat_name);

    // A dry run cannot learn whether a router already exists. Record the
    // create that a fresh deployment requires, matching the rest of setup's
    // create-oriented preview rather than inventing a synthetic drift patch.
    // The live drift path only ever adds this deployment's gateway, so the
    // preview omits no destructive step an operator would want to confirm.
    if client.mode() == Mode::DryRun {
        return create_router(client, project_id, region, &router_name, &nat_name, &body).await;
    }

    let response = client.get(GcpService::Compute, &path).await?;
    match response.status_u16() {
        404 => create_router(client, project_id, region, &router_name, &nat_name, &body).await,
        200..=299 => {
            let existing: serde_json::Value =
                serde_json::from_str(&response.into_text()).map_err(|source| SetupError::Json {
                    what: "Cloud Router lookup response",
                    source,
                })?;
            if !network_matches(&existing, &body) {
                return Err(SetupError::AmbiguousLiveState {
                    operation: format!("read Cloud Router {router_name}"),
                    detail: format!(
                        "a router named {router_name} already exists on a different network; \
                         refusing to move it onto {network_name}"
                    ),
                });
            }
            if has_expected_nat(&existing, &nat_name) {
                return Ok(());
            }
            let converged = nat_patch_body(&existing, &router_name, &nat_name);
            patch_router(client, project_id, region, &path, &router_name, &converged).await
        }
        status => Err(SetupError::BadStatus {
            operation: format!("read Cloud Router {router_name}"),
            status,
            body: response.into_text(),
        }),
    }
}

async fn create_router(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    router_name: &str,
    nat_name: &str,
    body: &serde_json::Value,
) -> SetupResult<()> {
    let collection = format!("/compute/v1/projects/{project_id}/regions/{region}/routers");
    let response = client
        .post_json(GcpService::Compute, &collection, body)
        .await?;
    match response.status_u16() {
        409 => {
            verify_router_after_conflict(client, project_id, region, router_name, nat_name, body)
                .await
        }
        200..=299 => {
            wait_for_router_operation(client, project_id, region, response.into_text()).await
        }
        status => Err(SetupError::BadStatus {
            operation: format!("create Cloud Router {router_name}"),
            status,
            body: response.into_text(),
        }),
    }
}

/// A create conflict only proves that another writer won the race. Read the
/// winning router before setup continues, so a router created without the
/// required NAT cannot be mistaken for converged private-node egress.
async fn verify_router_after_conflict(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    router_name: &str,
    nat_name: &str,
    expected: &serde_json::Value,
) -> SetupResult<()> {
    let path = router_path(project_id, region, router_name);
    let response = client.get(GcpService::Compute, &path).await?;
    match response.status_u16() {
        200..=299 => {
            let existing: serde_json::Value =
                serde_json::from_str(&response.into_text()).map_err(|source| SetupError::Json {
                    what: "Cloud Router lookup after create conflict",
                    source,
                })?;
            if router_has_expected_nat(&existing, expected, nat_name) {
                return Ok(());
            }
            Err(SetupError::AmbiguousLiveState {
                operation: format!("verify Cloud Router after create conflict {router_name}"),
                detail: format!(
                    "the winning router does not contain the expected NAT {nat_name}; refusing to overwrite its configuration"
                ),
            })
        }
        status => Err(SetupError::BadStatus {
            operation: format!("verify Cloud Router after create conflict {router_name}"),
            status,
            body: response.into_text(),
        }),
    }
}

async fn patch_router(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    path: &str,
    router_name: &str,
    body: &serde_json::Value,
) -> SetupResult<()> {
    let response = client.patch_json(GcpService::Compute, path, body).await?;
    match response.status_u16() {
        200..=299 => {
            wait_for_router_operation(client, project_id, region, response.into_text()).await
        }
        status => Err(SetupError::BadStatus {
            operation: format!("patch Cloud Router {router_name}"),
            status,
            body: response.into_text(),
        }),
    }
}

async fn wait_for_router_operation(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    response_body: String,
) -> SetupResult<()> {
    let operation: serde_json::Value =
        serde_json::from_str(&response_body).map_err(|source| SetupError::Json {
            what: "Cloud Router operation response",
            source,
        })?;
    lro::wait(
        client,
        GcpService::Compute,
        &operation,
        &format!("/compute/v1/projects/{project_id}/regions/{region}/operations/{{name}}"),
    )
    .await
    .map(|_| ())
}

/// Ensure a named custom-mode VPC without inheriting the production setup
/// configuration. Every deployment calls this seam so it cannot
/// acquire cluster or Config Sync settings by construction.
pub async fn ensure_named_network(
    client: &GcpClient,
    project_id: &str,
    network_name: &str,
) -> SetupResult<()> {
    ensure_named_network_with_retry(
        client,
        project_id,
        network_name,
        API_ACTIVATION_RETRY_INTERVAL,
        API_ACTIVATION_MAX_ATTEMPTS,
    )
    .await
}

async fn ensure_named_network_with_retry(
    client: &GcpClient,
    project_id: &str,
    network_name: &str,
    retry_interval: Duration,
    max_attempts: usize,
) -> SetupResult<()> {
    let body = json!({
        "name": network_name,
        "autoCreateSubnetworks": false,
        "routingConfig": { "routingMode": "REGIONAL" }
    });
    for attempt in 1..=max_attempts.max(1) {
        let resp = client
            .post_json(
                GcpService::Compute,
                &format!("/compute/v1/projects/{project_id}/global/networks"),
                &body,
            )
            .await?;
        let status = resp.status_u16();
        let response_body = resp.into_text();
        match status {
            409 => return Ok(()),
            200..=299 => {
                let op: serde_json::Value =
                    serde_json::from_str(&response_body).map_err(|source| SetupError::Json {
                        what: "network insert response",
                        source,
                    })?;
                lro::wait(
                    client,
                    GcpService::Compute,
                    &op,
                    &format!("/compute/v1/projects/{project_id}/global/operations/{{name}}"),
                )
                .await?;
                return Ok(());
            }
            403 if services::activation_is_propagating(
                &response_body,
                "compute.googleapis.com",
            ) && attempt < max_attempts.max(1) =>
            {
                eprintln!(
                    "gcp api [compute.googleapis.com] activation is still propagating for \
                     {project_id}; retrying VPC {network_name} ({attempt}/{max_attempts})"
                );
                if retry_interval.is_zero() {
                    tokio::task::yield_now().await;
                } else {
                    tokio::time::sleep(retry_interval).await;
                }
            }
            other => {
                return Err(SetupError::BadStatus {
                    operation: format!("create VPC {network_name}"),
                    status: other,
                    body: response_body,
                });
            }
        }
    }
    unreachable!("the retry loop always executes at least once")
}

/// Ensure the regional subnet a custom-mode VPC needs before GKE can select
/// it. Staging calls this directly rather than falling back to the project's
/// default network.
pub async fn ensure_named_subnetwork(
    client: &GcpClient,
    project_id: &str,
    region: &str,
    network_name: &str,
    subnetwork_name: &str,
) -> SetupResult<()> {
    let body = json!({
        "name": subnetwork_name,
        "network": format!("projects/{project_id}/global/networks/{network_name}"),
        "ipCidrRange": "10.82.0.0/20",
        "region": region,
        "privateIpGoogleAccess": true,
    });
    let resp = client
        .post_json(
            GcpService::Compute,
            &format!("/compute/v1/projects/{project_id}/regions/{region}/subnetworks"),
            &body,
        )
        .await?;
    let status = resp.status_u16();
    match status {
        409 => Ok(()),
        200..=299 => {
            let op: serde_json::Value =
                serde_json::from_str(&resp.into_text()).map_err(|source| SetupError::Json {
                    what: "subnetwork insert response",
                    source,
                })?;
            lro::wait(
                client,
                GcpService::Compute,
                &op,
                &format!("/compute/v1/projects/{project_id}/regions/{region}/operations/{{name}}"),
            )
            .await
            .map(|_| ())
        }
        other => Err(SetupError::BadStatus {
            operation: format!("create subnet {subnetwork_name}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::{GcpClient, GcpService, StaticToken};
    use super::super::{SetupConfig, SetupError};
    use super::{
        ensure_named_network_with_retry, ensure_network, ensure_router_and_nat, nat_name,
        router_name, DEFAULT_NETWORK_NAME,
    };

    fn client_for(server: &MockServer) -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Compute, server.uri())
    }

    #[tokio::test]
    async fn inserts_custom_mode_vpc_then_waits_for_lro() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/compute/v1/projects/p/global/networks"))
            .and(body_partial_json(json!({
                "name": DEFAULT_NETWORK_NAME,
                "autoCreateSubnetworks": false
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operation-123",
                "status": "RUNNING"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/compute/v1/projects/p/global/operations/operation-123",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operation-123",
                "status": "DONE"
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        ensure_network(&client, "p", &SetupConfig::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn inserts_subnet_then_polls_the_regional_operation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/compute/v1/projects/p/regions/us-west1/subnetworks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operation-456",
                "status": "PENDING"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/compute/v1/projects/p/regions/us-west1/operations/operation-456",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operation-456",
                "status": "DONE"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        super::ensure_named_subnetwork(
            &client,
            "p",
            "us-west1",
            DEFAULT_NETWORK_NAME,
            super::DEFAULT_SUBNETWORK_NAME,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn creates_a_router_with_auto_allocated_all_subnet_nat_then_polls() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let nat = nat_name(&config.cluster_name);
        let router_path = format!(
            "/compute/v1/projects/p/regions/{}/routers/{router}",
            config.region
        );
        Mock::given(method("GET"))
            .and(path(router_path))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/compute/v1/projects/p/regions/{}/routers",
                config.region
            )))
            .and(body_partial_json(json!({
                "name": router,
                "network": "projects/p/global/networks/navigator-vpc",
                "nats": [{
                    "name": nat,
                    "natIpAllocateOption": "AUTO_ONLY",
                    "sourceSubnetworkIpRangesToNat": "ALL_SUBNETWORKS_ALL_IP_RANGES",
                }],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "router-create",
                "status": "RUNNING"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/compute/v1/projects/p/regions/{}/operations/router-create",
                config.region
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "router-create",
                "status": "DONE"
            })))
            .expect(1)
            .mount(&server)
            .await;

        ensure_router_and_nat(&client_for(&server), "p", &config)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn patches_a_router_when_its_nat_settings_drift() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let nat = nat_name(&config.cluster_name);
        let router_path = format!(
            "/compute/v1/projects/p/regions/{}/routers/{router}",
            config.region
        );
        Mock::given(method("GET"))
            .and(path(&router_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "network": "projects/p/global/networks/navigator-vpc",
                "nats": [{
                    "name": nat,
                    "natIpAllocateOption": "MANUAL_ONLY",
                    "sourceSubnetworkIpRangesToNat": "LIST_OF_SUBNETWORKS",
                }],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(router_path))
            .and(body_partial_json(json!({
                "nats": [{
                    "name": nat,
                    "natIpAllocateOption": "AUTO_ONLY",
                    "sourceSubnetworkIpRangesToNat": "ALL_SUBNETWORKS_ALL_IP_RANGES",
                }],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "router-patch",
                "status": "DONE"
            })))
            .expect(1)
            .mount(&server)
            .await;

        ensure_router_and_nat(&client_for(&server), "p", &config)
            .await
            .unwrap();
    }

    /// `compute.routers.patch` replaces `nats` wholesale, so a convergence
    /// patch that carried only this deployment's gateway would silently delete
    /// every other one on a shared router.
    #[tokio::test]
    async fn drift_patch_preserves_nat_gateways_this_deployment_does_not_own() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let nat = nat_name(&config.cluster_name);
        let router_path = format!(
            "/compute/v1/projects/p/regions/{}/routers/{router}",
            config.region
        );
        Mock::given(method("GET"))
            .and(path(&router_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "network": "projects/p/global/networks/navigator-vpc",
                "bgp": { "asn": 65001 },
                "nats": [
                    {
                        "name": "unrelated-nat",
                        "natIpAllocateOption": "MANUAL_ONLY",
                        "sourceSubnetworkIpRangesToNat": "LIST_OF_SUBNETWORKS",
                    },
                    {
                        "name": nat,
                        "natIpAllocateOption": "MANUAL_ONLY",
                        "sourceSubnetworkIpRangesToNat": "LIST_OF_SUBNETWORKS",
                    },
                ],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(&router_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "router-patch",
                "status": "DONE"
            })))
            .expect(1)
            .mount(&server)
            .await;

        ensure_router_and_nat(&client_for(&server), "p", &config)
            .await
            .unwrap();

        let patch = server
            .received_requests()
            .await
            .expect("wiremock records the requests")
            .into_iter()
            .find(|request| request.method == wiremock::http::Method::PATCH)
            .expect("the drifted router receives a patch");
        let body: serde_json::Value =
            serde_json::from_slice(&patch.body).expect("the patch body is JSON");
        let nats = body["nats"].as_array().expect("the patch submits nats");
        assert_eq!(nats.len(), 2, "the unrelated gateway survives: {body}");
        assert_eq!(nats[0]["name"], "unrelated-nat", "{body}");
        assert_eq!(nats[0]["natIpAllocateOption"], "MANUAL_ONLY", "{body}");
        assert_eq!(nats[1]["name"], json!(nat), "{body}");
        assert_eq!(nats[1]["natIpAllocateOption"], "AUTO_ONLY", "{body}");
        assert_eq!(
            nats[1]["sourceSubnetworkIpRangesToNat"], "ALL_SUBNETWORKS_ALL_IP_RANGES",
            "{body}"
        );
        assert!(
            body.get("bgp").is_none(),
            "a drift patch must not rewrite the ASN of a router carrying real BGP sessions: {body}"
        );
    }

    /// Compute answers `routers.get` with a full selfLink while the create
    /// body carries the relative form. A verbatim comparison would call every
    /// converged router drifted and patch it on every run.
    #[tokio::test]
    async fn a_converged_router_returned_as_a_self_link_is_left_alone() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let nat = nat_name(&config.cluster_name);
        // No PATCH mock: wiremock 404s an unmatched request, so a patch here
        // fails the test.
        Mock::given(method("GET"))
            .and(path(format!(
                "/compute/v1/projects/p/regions/{}/routers/{router}",
                config.region
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "network":
                    "https://www.googleapis.com/compute/v1/projects/p/global/networks/navigator-vpc",
                "nats": [{
                    "name": nat,
                    "natIpAllocateOption": "AUTO_ONLY",
                    "sourceSubnetworkIpRangesToNat": "ALL_SUBNETWORKS_ALL_IP_RANGES",
                }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        ensure_router_and_nat(&client_for(&server), "p", &config)
            .await
            .unwrap();
    }

    /// A router of this name on another VPC belongs to somebody else. Setup
    /// stops rather than patching its gateway list.
    #[tokio::test]
    async fn stops_when_a_router_of_the_same_name_sits_on_another_network() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        Mock::given(method("GET"))
            .and(path(format!(
                "/compute/v1/projects/p/regions/{}/routers/{router}",
                config.region
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "network": "projects/p/global/networks/somebody-elses-vpc",
                "nats": [],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = ensure_router_and_nat(&client_for(&server), "p", &config).await;

        assert!(
            matches!(result, Err(SetupError::AmbiguousLiveState { .. })),
            "got {result:?}"
        );
    }

    #[tokio::test]
    async fn treats_a_router_create_conflict_as_success() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let nat = nat_name(&config.cluster_name);
        let router_path = format!(
            "/compute/v1/projects/p/regions/{}/routers/{router}",
            config.region
        );
        Mock::given(method("GET"))
            .and(path(router_path.clone()))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/compute/v1/projects/p/regions/{}/routers",
                config.region
            )))
            .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(router_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "network": "projects/p/global/networks/navigator-vpc",
                "nats": [{
                    "name": nat,
                    "natIpAllocateOption": "AUTO_ONLY",
                    "sourceSubnetworkIpRangesToNat": "ALL_SUBNETWORKS_ALL_IP_RANGES",
                }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        ensure_router_and_nat(&client_for(&server), "p", &config)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stops_when_a_router_create_conflict_wins_without_the_expected_nat() {
        let server = MockServer::start().await;
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let router_path = format!(
            "/compute/v1/projects/p/regions/{}/routers/{router}",
            config.region
        );
        Mock::given(method("GET"))
            .and(path(router_path.clone()))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/compute/v1/projects/p/regions/{}/routers",
                config.region
            )))
            .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(router_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "network": "projects/p/global/networks/navigator-vpc",
                "nats": [],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = ensure_router_and_nat(&client_for(&server), "p", &config).await;

        assert!(matches!(result, Err(SetupError::AmbiguousLiveState { .. })));
    }

    #[tokio::test]
    async fn treats_409_as_already_exists_and_skips_polling() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/compute/v1/projects/p/global/networks"))
            .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
            .expect(1)
            .mount(&server)
            .await;
        // No GET mock — if we tried to poll, wiremock would 404 the
        // call and fail the test.
        let client = client_for(&server);
        ensure_network(&client, "p", &SetupConfig::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retries_vpc_insert_while_compute_api_activation_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/compute/v1/projects/p/global/networks"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "error": {
                    "status": "PERMISSION_DENIED",
                    "details": [{
                        "reason": "SERVICE_DISABLED",
                        "metadata": {
                            "service": "compute.googleapis.com"
                        }
                    }]
                }
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/compute/v1/projects/p/global/networks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "operation-after-activation",
                "status": "DONE"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        ensure_named_network_with_retry(
            &client,
            "p",
            DEFAULT_NETWORK_NAME,
            std::time::Duration::ZERO,
            2,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_an_unrelated_compute_permission_denial() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/compute/v1/projects/p/global/networks"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "error": {
                    "status": "PERMISSION_DENIED",
                    "message": "caller lacks compute.networks.create"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        let err = ensure_named_network_with_retry(
            &client,
            "p",
            DEFAULT_NETWORK_NAME,
            std::time::Duration::ZERO,
            2,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err}").contains("caller lacks compute.networks.create"),
            "got {err}"
        );
    }

    #[tokio::test]
    async fn dry_run_records_only_the_post() {
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Compute, "http://127.0.0.1:1")
            .with_dry_run();
        ensure_network(&client, "p", &SetupConfig::default())
            .await
            .unwrap();
        let calls = client.recorded_calls();
        assert_eq!(
            calls.len(),
            1,
            "dry-run should only record the insert, got {calls:?}"
        );
        assert!(calls[0].url.ends_with("/global/networks"));
    }

    #[tokio::test]
    async fn dry_run_records_the_router_and_all_subnet_nat_create() {
        let client = GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Compute, "http://127.0.0.1:1")
            .with_dry_run();
        let config = SetupConfig::default();
        let router = router_name(&config.cluster_name);
        let nat = nat_name(&config.cluster_name);
        ensure_router_and_nat(&client, "p", &config).await.unwrap();

        let calls = client.recorded_calls();
        assert_eq!(
            calls.len(),
            1,
            "dry run records the router create: {calls:?}"
        );
        assert!(calls[0].url.ends_with("/regions/us-west4/routers"));
        let body = calls[0].body.as_deref().unwrap_or_default();
        assert!(body.contains(&router), "{body}");
        assert!(body.contains(&nat), "{body}");
        assert!(body.contains("AUTO_ONLY"), "{body}");
        assert!(body.contains("ALL_SUBNETWORKS_ALL_IP_RANGES"), "{body}");
    }
}
