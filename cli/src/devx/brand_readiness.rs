//! `navigator ops brand-readiness` — the bounded, per-host release-readiness
//! check ENG-808 requires.
//!
//! `cli::devx::ship` provisions a `ManagedCertificate` and an Ingress rule
//! for every registered brand (see `views::brand::release_brand_hosts`), not
//! every registered key — but a rendered manifest is a request, not proof.
//! A `kubectl apply` can succeed while a certificate still sits in
//! `Provisioning`, and a primary-host `200` (the old `ship::smoke_check`)
//! says nothing about the other hosts or the apex redirects. This command is
//! the receipt:
//! for every host the release inventory covers, on the selected deployment's
//! own environment, it performs a real TLS handshake through the host's
//! ordinary trust store (never `-k`/insecure) and inspects the actual
//! response — the same "ordinary trust/hostname validation" a browser does —
//! then reports Ready/not-Ready per host and exits nonzero if any host is
//! not.
//!
//! Every check is bounded: one `curl` call per host with `--max-time`, no
//! retry loop and no polling for a certificate to become `Active` — this is
//! a point-in-time receipt, safe to rerun as often as the operator likes
//! (`cargo run -p cli -- ops brand-readiness --deployment <name>`), not a
//! wait-until-ready gate.
//!
//! Every admitted host must show its own `og:site_name` — the same marker
//! `features/tests/brand_routing.rs` and `server/tests/routes.rs` already
//! grep for.

use std::process::Command;

use anyhow::{Context, Result};
use comfy_table::Table;

use views::brand::BrandKey;

use super::ship::ShipConfig;

/// One process invocation's result, abstracted so tests supply canned
/// `curl` output instead of dialing the network — the certificate-checker
/// sibling of `super::flake_hunt::CommandRunner`.
pub(crate) trait CommandRunner {
    fn run(&mut self, program: &str, args: &[String]) -> CommandOutcome;
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CommandOutcome {
    /// `None` when the process could not even be spawned (`curl` missing).
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) struct ProcessCommandRunner;

impl CommandRunner for ProcessCommandRunner {
    fn run(&mut self, program: &str, args: &[String]) -> CommandOutcome {
        match Command::new(program).args(args).output() {
            Ok(out) => CommandOutcome {
                exit_code: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(err) => CommandOutcome {
                exit_code: None,
                stdout: String::new(),
                stderr: err.to_string(),
            },
        }
    }
}

/// The bounded outcome one host or apex check reaches. `Ready` is the only
/// passing variant; every other variant fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// An admitted brand served its own branded `200`, or an apex correctly
    /// redirected home.
    Ready,
    /// DNS did not resolve, or the connection was refused — `curl` exit 6/7.
    Unreachable,
    /// The bounded `--max-time` elapsed before the transaction completed.
    Timeout,
    /// The TLS handshake itself failed: untrusted chain, expired
    /// certificate, hostname/SAN mismatch, or the certificate is still
    /// `Provisioning` (which, from outside the cluster, looks identical to
    /// "no valid certificate yet" — this run cannot and does not distinguish
    /// pending from failed issuance without cluster access; it only proves
    /// the apex is not yet safe to call ready).
    CertificateNotReady,
    /// A brand's host answered but not with its own branded `200`.
    WrongBrandContent,
    /// Retained for readiness compatibility with deployments that still have
    /// a held-out registry entry.
    UnexpectedlyLive,
    /// An apex either did not redirect or redirected somewhere other than
    /// its own brand's canonical host.
    RedirectFailure,
    /// A registered host answered with a status this check has no other
    /// bucket for.
    UnexpectedStatus,
}

impl Verdict {
    #[must_use]
    pub(crate) const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// One host or apex's result: which brand it belongs to, the name checked,
/// the verdict, and a human-readable line naming why.
#[derive(Debug, Clone)]
pub(crate) struct HostReport {
    pub key: BrandKey,
    pub name: &'static str,
    pub verdict: Verdict,
    pub detail: String,
}

/// `curl` exit codes that mean the TLS handshake or certificate itself is
/// the problem — untrusted, expired, hostname/SAN mismatch, or otherwise
/// refused by the ordinary trust store curl carries. Never bypassed with
/// `-k`; this list exists so a *real* certificate problem is reported as
/// one instead of falling into the generic unreachable bucket.
const TLS_FAILURE_EXIT_CODES: &[i32] = &[35, 51, 58, 59, 60, 66, 77, 82, 83, 90, 91];

fn classify_transport_failure(outcome: &CommandOutcome) -> Option<Verdict> {
    match outcome.exit_code {
        Some(0) => None,
        Some(6 | 7) => Some(Verdict::Unreachable),
        Some(28) => Some(Verdict::Timeout),
        Some(code) if TLS_FAILURE_EXIT_CODES.contains(&code) => Some(Verdict::CertificateNotReady),
        _ => Some(Verdict::Unreachable),
    }
}

/// The first `HTTP/… <code>` status line `curl -D -` dumps, parsed. `curl`
/// dumps one such line per hop (redirects, 100-continue); the first is the
/// only one these checks ever need, since neither call here follows
/// redirects (`-L` is never passed).
fn parse_status_code(headers: &str) -> Option<u16> {
    headers
        .lines()
        .find(|line| line.starts_with("HTTP/"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
}

/// The value of a `curl -D -` dumped header, case-insensitively, with any
/// trailing `\r` and surrounding whitespace trimmed.
fn parse_header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().trim_end_matches('\r'))
    })
}

/// Whether `body` declares `site_name` as its `og:site_name` — the same
/// marker and both attribute-order spellings `features/tests/brand_routing.rs`
/// and `server/tests/routes.rs` already assert against.
fn declares_brand(body: &str, site_name: &str) -> bool {
    body.contains(&format!("og:site_name\" content=\"{site_name}\""))
        || body.contains(&format!(
            "content=\"{site_name}\" property=\"og:site_name\""
        ))
}

/// Every (key, host) pair this deployment's environment must prove ready:
/// the full release inventory (`views::brand::release_brand_hosts`) that
/// matches `public_host`'s own `staging.`-prefix convention — the same split
/// `cli::devx::ship::additional_brand_bindings` reads.
pub(crate) fn release_targets(public_host: &str) -> Vec<(BrandKey, &'static str)> {
    let staging = public_host.starts_with("staging.");
    views::brand::release_brand_hosts()
        .into_iter()
        .filter(|(_, host)| host.starts_with("staging.") == staging)
        .collect()
}

/// Check one registered host: a real TLS handshake and HTTP `GET /` through
/// the ordinary trust store, then the brand-appropriate expectation — a
/// an admitted brand's own `og:site_name` at `200`.
pub(crate) fn check_host<R: CommandRunner>(
    runner: &mut R,
    key: BrandKey,
    host: &'static str,
    expected_site_name: &str,
    timeout_secs: u32,
) -> HostReport {
    let url = format!("https://{host}/");
    let outcome = runner.run(
        "curl",
        &[
            "-sS".into(),
            "--max-time".into(),
            timeout_secs.to_string(),
            "-D".into(),
            "-".into(),
            url,
        ],
    );
    if let Some(verdict) = classify_transport_failure(&outcome) {
        return HostReport {
            key,
            name: host,
            verdict,
            detail: format!(
                "curl exit {:?}: {}",
                outcome.exit_code,
                outcome.stderr.trim()
            ),
        };
    }
    let status = parse_status_code(&outcome.stdout);
    if key.is_live() {
        if status == Some(200) && declares_brand(&outcome.stdout, expected_site_name) {
            return HostReport {
                key,
                name: host,
                verdict: Verdict::Ready,
                detail: format!("200, og:site_name={expected_site_name:?}"),
            };
        }
        return HostReport {
            key,
            name: host,
            verdict: Verdict::WrongBrandContent,
            detail: format!("expected 200 branded {expected_site_name:?}, got status {status:?}"),
        };
    }
    match status {
        Some(404) => HostReport {
            key,
            name: host,
            verdict: Verdict::Ready,
            detail: "404 (held out, as expected — not yet launched)".to_string(),
        },
        Some(200) => HostReport {
            key,
            name: host,
            verdict: Verdict::UnexpectedlyLive,
            detail: "held-out brand served 200 — host-admission regression".to_string(),
        },
        _ => HostReport {
            key,
            name: host,
            verdict: Verdict::UnexpectedStatus,
            detail: format!("expected 404 (held out), got {status:?}"),
        },
    }
}

/// Check one live brand's apex: it must redirect to that brand's own
/// canonical `www` host over a trusted HTTPS handshake — never a TLS error,
/// and never another brand's host. Held-out brands are skipped: their DNS is
/// not expected to point anywhere yet (see `views::brand::BrandKey::apex`).
pub(crate) fn check_apex_redirect<R: CommandRunner>(
    runner: &mut R,
    key: BrandKey,
    timeout_secs: u32,
) -> HostReport {
    let apex = key.apex();
    let url = format!("https://{apex}/");
    let outcome = runner.run(
        "curl",
        &[
            "-sS".into(),
            "--max-time".into(),
            timeout_secs.to_string(),
            "-D".into(),
            "-".into(),
            "-o".into(),
            "/dev/null".into(),
            url,
        ],
    );
    if let Some(verdict) = classify_transport_failure(&outcome) {
        return HostReport {
            key,
            name: apex,
            verdict,
            detail: format!(
                "curl exit {:?}: {}",
                outcome.exit_code,
                outcome.stderr.trim()
            ),
        };
    }
    let status = parse_status_code(&outcome.stdout);
    let location = parse_header(&outcome.stdout, "location");
    let expected = format!("https://{}/", key.canonical_host());
    let redirected_home =
        location.is_some_and(|loc| loc.trim_end_matches('/') == expected.trim_end_matches('/'));
    if matches!(status, Some(301 | 302 | 308)) && redirected_home {
        HostReport {
            key,
            name: apex,
            verdict: Verdict::Ready,
            detail: format!("redirects to {expected}"),
        }
    } else {
        HostReport {
            key,
            name: apex,
            verdict: Verdict::RedirectFailure,
            detail: format!(
                "expected a redirect to {expected}, got status {status:?} location {location:?}"
            ),
        }
    }
}

/// Run every check this deployment's environment owes: every release-
/// inventory host, plus (production only) every live brand's apex redirect.
pub(crate) fn run_with<R: CommandRunner>(
    runner: &mut R,
    cfg: &ShipConfig,
    timeout_secs: u32,
) -> Vec<HostReport> {
    let mut reports: Vec<HostReport> = release_targets(&cfg.public_host)
        .into_iter()
        .map(|(key, host)| {
            let expected_site_name = key
                .resolve_branding(&views::brand::DEFAULT_BRANDING)
                .firm
                .site_name;
            check_host(runner, key, host, expected_site_name, timeout_secs)
        })
        .collect();
    // The apex redirect always targets production `www` (`views::brand::
    // BrandKey::apex`/`canonical_host`), independent of which environment is
    // being checked — so it is only meaningful, and only expected to be
    // wired up, on the production run.
    if !cfg.public_host.starts_with("staging.") {
        for key in views::brand::BrandKey::ALL
            .iter()
            .copied()
            .filter(|k| k.is_live())
        {
            reports.push(check_apex_redirect(runner, key, timeout_secs));
        }
    }
    reports
}

fn print_report(cfg: &ShipConfig, reports: &[HostReport]) {
    let mut table = Table::new();
    table.set_header(vec!["brand", "host", "verdict", "detail"]);
    for report in reports {
        table.add_row(vec![
            report.key.as_str().to_string(),
            report.name.to_string(),
            format!("{:?}", report.verdict),
            report.detail.clone(),
        ]);
    }
    eprintln!("==> brand release readiness for {}\n{table}", cfg.name);
}

/// `ops brand-readiness --deployment <name>`: the real entry point. Bounded,
/// safe to rerun, and non-mutating — it never issues, orders, or applies
/// anything; see `ops dns setup --redirect-apex-to-www` and `ops ship` for
/// the commands that provision what this one only verifies.
pub fn run(cfg: &ShipConfig, timeout_secs: u32) -> Result<()> {
    let mut runner = ProcessCommandRunner;
    let reports = run_with(&mut runner, cfg, timeout_secs);
    print_report(cfg, &reports);
    let failed: Vec<&HostReport> = reports.iter().filter(|r| !r.verdict.is_ready()).collect();
    if failed.is_empty() {
        eprintln!(
            "==> all {} release-readiness checks passed for {}",
            reports.len(),
            cfg.name
        );
        return Ok(());
    }
    anyhow::bail!(
        "{} of {} release-readiness checks failed for {}: {}",
        failed.len(),
        reports.len(),
        cfg.name,
        failed
            .iter()
            .map(|r| format!("{} ({:?})", r.name, r.verdict))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `navigator ops brand-readiness`: resolve the deployment, then run.
pub fn run_for_deployment(
    deployment: &str,
    deployments_dir: Option<&std::path::Path>,
    timeout_secs: u32,
) -> Result<()> {
    let root = super::deployments::root(deployments_dir)?;
    let loaded = super::deployments::Deployment::load(&root, deployment)?;
    let cfg = ShipConfig::from_deployment(&loaded)
        .with_context(|| format!("resolve ShipConfig for deployment {deployment}"))?;
    run(&cfg, timeout_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted [`CommandRunner`]: each call to `run` pops the next
    /// programmed outcome, in order. Panics if more calls happen than were
    /// programmed — a test that doesn't script every call it expects is a
    /// test that doesn't know what it's actually asserting.
    struct ScriptedRunner {
        outcomes: std::collections::VecDeque<CommandOutcome>,
        calls: Vec<(String, Vec<String>)>,
    }

    impl ScriptedRunner {
        fn new(outcomes: Vec<CommandOutcome>) -> Self {
            Self {
                outcomes: outcomes.into(),
                calls: Vec::new(),
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&mut self, program: &str, args: &[String]) -> CommandOutcome {
            self.calls.push((program.to_string(), args.to_vec()));
            self.outcomes
                .pop_front()
                .expect("more curl calls happened than this test scripted")
        }
    }

    fn ok_headers_and_body(status: u16, body: &str) -> CommandOutcome {
        CommandOutcome {
            exit_code: Some(0),
            stdout: format!("HTTP/1.1 {status} OK\r\ncontent-type: text/html\r\n\r\n{body}"),
            stderr: String::new(),
        }
    }

    fn transport_failure(exit_code: i32, stderr: &str) -> CommandOutcome {
        CommandOutcome {
            exit_code: Some(exit_code),
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// A minimal [`ShipConfig`] naming only the field [`run_with`] actually
    /// reads (`public_host`); every other field is an arbitrary valid value.
    fn ship_config_for(public_host: &str) -> ShipConfig {
        ShipConfig {
            name: "test-deployment".into(),
            environment: store::DeploymentEnvironment::Dev,
            project_id: "test-project".into(),
            location: "us-west4".into(),
            cluster: "navigator".into(),
            registry: "ghcr.io/neon-law-source-code".into(),
            namespace: "navigator".into(),
            web_image_name: "neon-server".into(),
            public_host: public_host.to_string(),
            asset_base_url: format!("https://{public_host}/assets"),
            google_service_account_id: "navigator-web".into(),
            primary_domain: "neonlaw.com".into(),
            secret_name: "navigator-web-secrets".into(),
            workflows_url: None,
            context: "gke_test-project_us-west4_navigator".into(),
        }
    }

    #[test]
    fn release_targets_covers_the_full_registry_not_only_live_brands() {
        let production = release_targets("www.neonlaw.com");
        assert_eq!(production.len(), views::brand::BrandKey::ALL.len());
        assert!(production.iter().any(|(key, _)| !key.is_live()));
        assert!(production.contains(&(BrandKey::Daybridge, "www.daybridgedivorce.com")));
        let staging = release_targets("staging.neonlaw.com");
        assert!(staging.iter().all(|(_, host)| host.starts_with("staging.")));
    }

    #[test]
    fn a_live_brand_with_the_right_content_is_ready() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(
            200,
            r#"<meta property="og:site_name" content="Neon Law">"#,
        )]);
        let report = check_host(
            &mut runner,
            BrandKey::Neon,
            "www.neonlaw.com",
            "Neon Law",
            10,
        );
        assert_eq!(report.verdict, Verdict::Ready);
    }

    #[test]
    fn a_live_brand_serving_the_wrong_brand_content_fails() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(
            200,
            r#"<meta property="og:site_name" content="Some Other Brand">"#,
        )]);
        let report = check_host(
            &mut runner,
            BrandKey::Neon,
            "www.neonlaw.com",
            "Neon Law",
            10,
        );
        assert_eq!(report.verdict, Verdict::WrongBrandContent);
    }

    #[test]
    fn a_live_brand_returning_a_non_200_fails() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(500, "internal error")]);
        let report = check_host(
            &mut runner,
            BrandKey::Neon,
            "www.neonlaw.com",
            "Neon Law",
            10,
        );
        assert_eq!(report.verdict, Verdict::WrongBrandContent);
    }

    #[test]
    fn summons_requires_its_public_holding_page() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(404, "not found")]);
        let report = check_host(
            &mut runner,
            BrandKey::Summons,
            "www.summonsdefense.nyc",
            "Shook Law PLLC",
            10,
        );
        assert_eq!(report.verdict, Verdict::WrongBrandContent);
    }

    #[test]
    fn summons_answering_200_in_its_own_brand_is_ready() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(
            200,
            r#"<meta property="og:site_name" content="Shook Law PLLC">"#,
        )]);
        let report = check_host(
            &mut runner,
            BrandKey::Summons,
            "www.summonsdefense.nyc",
            "Shook Law PLLC",
            10,
        );
        assert_eq!(report.verdict, Verdict::Ready);
    }

    #[test]
    fn summons_answering_500_fails() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(500, "boom")]);
        let report = check_host(
            &mut runner,
            BrandKey::Summons,
            "www.summonsdefense.nyc",
            "Shook Law PLLC",
            10,
        );
        assert_eq!(report.verdict, Verdict::WrongBrandContent);
    }

    #[test]
    fn dns_conflict_or_missing_host_is_unreachable() {
        for exit_code in [6, 7] {
            let mut runner =
                ScriptedRunner::new(vec![transport_failure(exit_code, "Could not resolve host")]);
            let report = check_host(
                &mut runner,
                BrandKey::Neon,
                "www.neonlaw.com",
                "Neon Law",
                10,
            );
            assert_eq!(report.verdict, Verdict::Unreachable, "exit {exit_code}");
        }
    }

    #[test]
    fn a_timeout_is_reported_as_a_timeout_not_unreachable() {
        let mut runner = ScriptedRunner::new(vec![transport_failure(28, "Operation timed out")]);
        let report = check_host(
            &mut runner,
            BrandKey::Neon,
            "www.neonlaw.com",
            "Neon Law",
            10,
        );
        assert_eq!(report.verdict, Verdict::Timeout);
    }

    #[test]
    fn untrusted_expired_or_wrong_san_certificates_fail_as_certificate_not_ready() {
        // 60 = SSL cert problem (untrusted/expired), 51 = peer cert cannot be
        // authenticated (name/SAN mismatch) — the two ways a real handshake
        // proves the certificate is not actually ready, distinct from a
        // plain network failure.
        for exit_code in [51, 60] {
            let mut runner = ScriptedRunner::new(vec![transport_failure(
                exit_code,
                "SSL certificate problem",
            )]);
            let report = check_host(
                &mut runner,
                BrandKey::Neon,
                "www.neonlaw.com",
                "Neon Law",
                10,
            );
            assert_eq!(
                report.verdict,
                Verdict::CertificateNotReady,
                "exit {exit_code}"
            );
        }
    }

    #[test]
    fn never_disables_tls_verification() {
        // The load-bearing negative: no `-k`/`--insecure` flag ever leaves
        // this module, for either check. A readiness check that turns off
        // verification to get past a certificate problem would defeat the
        // entire point of running it.
        let mut runner = ScriptedRunner::new(vec![
            ok_headers_and_body(200, r#"<meta property="og:site_name" content="Neon Law">"#),
            ok_headers_and_body(301, ""),
        ]);
        check_host(
            &mut runner,
            BrandKey::Neon,
            "www.neonlaw.com",
            "Neon Law",
            10,
        );
        check_apex_redirect(&mut runner, BrandKey::Neon, 10);
        for (_, args) in &runner.calls {
            assert!(
                !args.iter().any(|arg| arg == "-k" || arg == "--insecure"),
                "must never disable TLS verification: {args:?}"
            );
        }
    }

    #[test]
    fn a_correct_apex_redirect_is_ready() {
        let mut runner = ScriptedRunner::new(vec![CommandOutcome {
            exit_code: Some(0),
            stdout: "HTTP/1.1 301 Moved Permanently\r\nlocation: https://www.neonlaw.com/\r\n\r\n"
                .to_string(),
            stderr: String::new(),
        }]);
        let report = check_apex_redirect(&mut runner, BrandKey::Neon, 10);
        assert_eq!(report.verdict, Verdict::Ready);
    }

    #[test]
    fn a_redirect_to_the_wrong_host_fails() {
        let mut runner = ScriptedRunner::new(vec![CommandOutcome {
            exit_code: Some(0),
            stdout:
                "HTTP/1.1 301 Moved Permanently\r\nlocation: https://www.someone-else.com/\r\n\r\n"
                    .to_string(),
            stderr: String::new(),
        }]);
        let report = check_apex_redirect(&mut runner, BrandKey::Neon, 10);
        assert_eq!(report.verdict, Verdict::RedirectFailure);
    }

    #[test]
    fn a_missing_redirect_fails() {
        let mut runner = ScriptedRunner::new(vec![ok_headers_and_body(200, "no redirect here")]);
        let report = check_apex_redirect(&mut runner, BrandKey::Neon, 10);
        assert_eq!(report.verdict, Verdict::RedirectFailure);
    }

    #[test]
    fn run_with_checks_apexes_only_on_the_production_environment() {
        // Staging: one call per release-inventory host (8), no apex calls.
        let mut staging_runner = ScriptedRunner::new(
            (0..views::brand::BrandKey::ALL.len())
                .map(|_| ok_headers_and_body(200, r#"content="x" property="og:site_name""#))
                .collect(),
        );
        let staging_cfg = ship_config_for("staging.neonlaw.com");
        let staging_reports = run_with(&mut staging_runner, &staging_cfg, 10);
        assert_eq!(staging_reports.len(), views::brand::BrandKey::ALL.len());

        // Production: the same host checks, plus one apex check per brand.
        let host_count = views::brand::BrandKey::ALL.len();
        let live_count = views::brand::BrandKey::LIVE.len();
        let mut outcomes: Vec<CommandOutcome> = (0..host_count)
            .map(|_| ok_headers_and_body(200, r#"content="x" property="og:site_name""#))
            .collect();
        outcomes.extend((0..live_count).map(|_| CommandOutcome {
            exit_code: Some(0),
            stdout: "HTTP/1.1 301 Moved Permanently\r\nlocation: https://x/\r\n\r\n".to_string(),
            stderr: String::new(),
        }));
        let mut production_runner = ScriptedRunner::new(outcomes);
        let production_cfg = ship_config_for("www.neonlaw.com");
        let production_reports = run_with(&mut production_runner, &production_cfg, 10);
        assert_eq!(production_reports.len(), host_count + live_count);
    }

    #[test]
    fn repeat_execution_is_a_pure_read_with_no_state_carried_between_runs() {
        // Running the whole check twice against an unchanged, all-ready
        // world must report the same thing both times — no memoization, no
        // retry state leaking across runs.
        let script = || {
            (0..views::brand::BrandKey::ALL.len())
                .map(|_| ok_headers_and_body(200, r#"content="x" property="og:site_name""#))
                .collect()
        };
        let cfg = ship_config_for("staging.neonlaw.com");
        let mut first_runner = ScriptedRunner::new(script());
        let first = run_with(&mut first_runner, &cfg, 10);
        let mut second_runner = ScriptedRunner::new(script());
        let second = run_with(&mut second_runner, &cfg, 10);
        assert_eq!(
            first.iter().map(|r| r.verdict).collect::<Vec<_>>(),
            second.iter().map(|r| r.verdict).collect::<Vec<_>>(),
        );
    }
}
