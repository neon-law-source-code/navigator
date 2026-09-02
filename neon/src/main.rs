//! The `neon-server` binary: `www.neonlaw.com`.
//!
//! The composition it serves lives in the crate's library, so this binary and
//! the tests that exercise its router cannot disagree. See [`neon::brand`].

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    portal::hosting::run(neon::brand()).await
}
