use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::protocol::{AdapterReply, AdapterRequest};

const ADAPTER_ENV: &str = "NAVIGATOR_WORD_ADAPTER";
const DEFAULT_ADAPTER: &str = "navigator-word-adapter";
const ADAPTER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROTOCOL_BYTES: usize = 64 * 1024 * 1024;

/// The Rust/managed parsing seam. The adapter receives one bounded JSON
/// request on stdin and returns one bounded JSON response on stdout.
#[async_trait]
pub trait WordAdapter: Send + Sync {
    async fn parse(&self, request: AdapterRequest) -> Result<AdapterReply, AdapterError>;
}

/// Local process adapter built by the pinned managed-runtime image stage.
#[derive(Debug, Clone)]
pub struct ManagedAdapter {
    program: PathBuf,
}

impl ManagedAdapter {
    /// Resolve the adapter executable from the deployment environment. No
    /// network or package restore is performed by this constructor.
    #[must_use]
    pub fn from_env() -> Self {
        let program = std::env::var_os(ADAPTER_ENV)
            .map_or_else(|| PathBuf::from(DEFAULT_ADAPTER), PathBuf::from);
        Self { program }
    }

    #[must_use]
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

#[async_trait]
impl WordAdapter for ManagedAdapter {
    async fn parse(&self, request: AdapterRequest) -> Result<AdapterReply, AdapterError> {
        let input = serde_json::to_vec(&request).map_err(AdapterError::Serialize)?;
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        let mut child = command.spawn().map_err(|_| AdapterError::Unavailable)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&input)
                .await
                .map_err(|_| AdapterError::Unavailable)?;
        }
        let output = timeout(ADAPTER_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| AdapterError::TimedOut)?
            .map_err(|_| AdapterError::Unavailable)?;
        if output.stdout.len() > MAX_PROTOCOL_BYTES {
            return Err(AdapterError::ResponseTooLarge);
        }
        let reply: AdapterReply =
            serde_json::from_slice(&output.stdout).map_err(|_| AdapterError::InvalidResponse)?;
        Ok(reply)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("adapter request serialization failed")]
    Serialize(#[source] serde_json::Error),
    #[error("adapter executable unavailable")]
    Unavailable,
    #[error("adapter timed out")]
    TimedOut,
    #[error("adapter response exceeded the protocol limit")]
    ResponseTooLarge,
    #[error("adapter returned an invalid response")]
    InvalidResponse,
}
