//! Read-only access to the Solana Attestation Service program on devnet.
//!
//! The official generated SAS client supplies the program identity. Solana
//! JSON-RPC supplies the finalized account observation. Transaction
//! construction and submission belong behind Navigator's authenticated REST
//! command boundary, not in an operator probe.

use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use solana_attestation_service_client::programs::SOLANA_ATTESTATION_SERVICE_ID;

const DEVNET_RPC_URL: &str = "https://api.devnet.solana.com";

/// Read the SAS program account from devnet at finalized commitment.
pub async fn program() -> Result<ExitCode> {
    let response = reqwest::Client::new()
        .post(DEVNET_RPC_URL)
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
