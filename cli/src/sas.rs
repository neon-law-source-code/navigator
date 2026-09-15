//! Read-only access to the Solana Attestation Service program on devnet.
//!
//! The official generated SAS client supplies the program identity. Solana
//! JSON-RPC supplies the finalized account observation. Transaction
//! construction and submission belong behind Navigator's authenticated REST
//! command boundary, not in an operator probe.

use std::{process::ExitCode, time::Duration};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use solana_attestation_service_client::programs::SOLANA_ATTESTATION_SERVICE_ID;

const DEVNET_RPC_URL: &str = "https://api.devnet.solana.com";
const DEVNET_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEVNET_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Read the SAS program account from devnet at finalized commitment.
pub async fn program() -> Result<ExitCode> {
    program_with_client(&devnet_client()?, DEVNET_RPC_URL).await
}

fn devnet_client() -> Result<reqwest::Client> {
    rpc_client(DEVNET_CONNECT_TIMEOUT, DEVNET_REQUEST_TIMEOUT)
}

fn rpc_client(connect_timeout: Duration, request_timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .context("build the Solana devnet RPC client")
}

async fn program_with_client(client: &reqwest::Client, endpoint: &str) -> Result<ExitCode> {
    let response = client
        .post(endpoint)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [
                SOLANA_ATTESTATION_SERVICE_ID.to_string(),
                { "commitment": "finalized", "encoding": "base64" }
            ]
        }))
        .send()
        .await
        .context("read the Solana devnet RPC")?
        .error_for_status()
        .context("Solana devnet RPC returned an HTTP error")?
        .json::<Value>()
        .await
        .context("decode the Solana devnet RPC response")?;

    if let Some(error) = response.get("error") {
        bail!("Solana devnet RPC error: {error}");
    }

    let value = response
        .pointer("/result/value")
        .context("Solana devnet did not return the SAS program account")?;
    let lamports = value
        .get("lamports")
        .and_then(Value::as_u64)
        .context("SAS program account omitted lamports")?;
    let executable = value
        .get("executable")
        .and_then(Value::as_bool)
        .context("SAS program account omitted executable")?;
    let data_size_bytes = value
        .pointer("/data/0")
        .and_then(Value::as_str)
        .map(|data| STANDARD.decode(data).map(|bytes| bytes.len()))
        .transpose()
        .context("SAS program account contained invalid base64 data")?
        .context("SAS program account omitted base64 data")?;

    println!("cluster: devnet");
    println!("commitment: finalized");
    println!("program: {SOLANA_ATTESTATION_SERVICE_ID}");
    println!("executable: {executable}");
    println!("lamports: {lamports}");
    println!("account_size_bytes: {data_size_bytes}");

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use wiremock::{
        matchers::{body_partial_json, method},
        Mock, MockServer, ResponseTemplate,
    };

    use super::*;

    fn account_response() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "value": {
                    "lamports": 1,
                    "executable": true,
                    "data": ["YQ==", "base64"],
                },
            },
        })
    }

    #[test]
    fn devnet_probe_uses_five_second_connect_and_fifteen_second_request_bounds() {
        assert_eq!(DEVNET_CONNECT_TIMEOUT, Duration::from_secs(5));
        assert_eq!(DEVNET_REQUEST_TIMEOUT, Duration::from_secs(15));
    }

    #[tokio::test]
    async fn a_valid_rpc_response_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({
                "method": "getAccountInfo",
                "params": [
                    SOLANA_ATTESTATION_SERVICE_ID.to_string(),
                    { "commitment": "finalized", "encoding": "base64" },
                ],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(account_response()))
            .mount(&server)
            .await;

        let client = rpc_client(Duration::from_secs(1), Duration::from_secs(1))
            .expect("test RPC client builds");
        assert_eq!(
            program_with_client(&client, &server.uri())
                .await
                .expect("valid RPC response succeeds"),
            ExitCode::SUCCESS,
        );
    }

    #[tokio::test]
    async fn an_http_error_remains_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = rpc_client(Duration::from_secs(1), Duration::from_secs(1))
            .expect("test RPC client builds");
        let error = program_with_client(&client, &server.uri())
            .await
            .expect_err("an HTTP error must remain an error");
        assert!(
            error.to_string().contains("HTTP error"),
            "the HTTP status failure stays visible: {error}",
        );
    }

    #[tokio::test]
    async fn an_rpc_error_remains_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": -32000, "message": "fixture failure" },
            })))
            .mount(&server)
            .await;

        let client = rpc_client(Duration::from_secs(1), Duration::from_secs(1))
            .expect("test RPC client builds");
        let error = program_with_client(&client, &server.uri())
            .await
            .expect_err("an RPC error must remain an error");
        assert!(
            error.to_string().contains("Solana devnet RPC error"),
            "the RPC error stays visible: {error}",
        );
    }

    #[tokio::test]
    async fn a_server_that_accepts_but_withholds_its_response_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture server");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("accept client connection");
            accepted_tx.send(()).expect("record accepted connection");
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let client = rpc_client(Duration::from_millis(20), Duration::from_millis(50))
            .expect("test RPC client builds");

        let started = Instant::now();
        let error = program_with_client(&client, &endpoint)
            .await
            .expect_err("a withheld response must time out");
        accepted_rx
            .await
            .expect("the fixture accepted the request before it timed out");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the request must be bounded rather than waiting for the fixture server"
        );
        assert!(
            error.to_string().contains("read the Solana devnet RPC"),
            "the timeout retains the request context: {error}",
        );
        server.await.expect("fixture server completes");
    }
}
