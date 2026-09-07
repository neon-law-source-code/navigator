//! Proves `crypto_provider_is_healthy` actually detects the ENG-550
//! regression it exists to catch (ENG-551): in a process that has never
//! called `install_crypto_provider` — exactly the state this binary was in
//! before ENG-550's fix — `jsonwebtoken`'s process-level `CryptoProvider` is
//! still ambiguous (both `rust_crypto`, from `restate-jwt`, and `aws_lc_rs`,
//! from `store`'s `surrealdb-core` dependency, are compiled in), so the
//! health check must report `false` instead of panicking the probe itself.
//! Runs in its own process (a separate binary from every other test in this
//! crate) so no other test's `install_crypto_provider()` call can mask this.

use workflows_service::request_identity::crypto_provider_is_healthy;

#[test]
fn health_check_reports_unhealthy_when_the_provider_was_never_pinned() {
    assert!(!crypto_provider_is_healthy());
}
