//! Typed deployment-invariant metadata shared by service startup and
//! `navigator ops ship` preflight.

use crate::DeploymentEnvironment;

pub const CREDENTIAL_ENVIRONMENT: &str = "NAVIGATOR_CREDENTIAL_ENVIRONMENT";
pub const CI_HARNESS: &str = "NAVIGATOR_CI_HARNESS";
/// The one GCP project authorized to consume the global engineering webhook.
///
/// `neon-law-stg` rather than a production project on purpose: engineering
/// automation acts on this repository, not on anyone's matters, so it belongs
/// in the deployment whose data plane is sample by construction.
pub const GITHUB_AUTOMATION_HOME_PROJECT: &str = "neon-law-stg";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    pub any_of: &'static [&'static [&'static str]],
    pub trigger: Option<&'static str>,
    pub integration: bool,
    /// Restrict this requirement to one GCP project. `None` applies to every
    /// deployment.
    pub project_id: Option<&'static str>,
}

macro_rules! required {
    ($key:literal) => {
        Requirement {
            any_of: &[&[$key]],
            trigger: None,
            integration: false,
            project_id: None,
        }
    };
    (integration $key:literal) => {
        Requirement {
            any_of: &[&[$key]],
            trigger: None,
            integration: true,
            project_id: None,
        }
    };
    // An integration a deployment may decline entirely, but may not
    // half-configure: required only once `$trigger` declares the integration
    // present.
    (integration $key:literal if $trigger:literal) => {
        Requirement {
            any_of: &[&[$key]],
            trigger: Some($trigger),
            integration: true,
            project_id: None,
        }
    };
}

/// Web boot requirements whose key shape is also consumed by `ops ship`.
pub static WEB_REQUIREMENTS: &[Requirement] = &[
    required!("RESTATE_BROKER_URL"),
    required!("NAVIGATOR_STORAGE_BACKEND"),
    // The private bucket each Project's published client-portal bundle lives in,
    // streamed same-origin through `/app/projects/{code}/portal`. Mandatory like
    // the other object-storage lanes: `ops gcp setup` provisions it for every
    // deployment, so a deployment missing the coordinate is a real gap `ops ship`
    // must refuse rather than a lane it may decline.
    required!("NAVIGATOR_APPLICATIONS_BUCKET"),
    required!("NAVIGATOR_CLAMD_ADDR"),
    required!(integration "NAVIGATOR_EMAIL_BACKEND"),
    required!(integration "SENDGRID_API_KEY"),
    required!(integration "SENDGRID_FROM_EMAIL"),
    required!(integration "SENDGRID_INBOUND_SECRET"),
    required!(integration "SENDGRID_EVENTS_SECRET"),
    required!(integration "SENDGRID_EVENTS_PUBLIC_KEY"),
    // DocuSign is declared by `DOCUSIGN_BASE_URL` and is otherwise absent.
    //
    // A deployment that executes no documents supplies none of these and runs
    // `StubSignatureProvider`. The stub is reachable only through genuine
    // absence: `portal::signature::DocuSignSignatureProvider::from_env`
    // returns `Some` for any non-empty value, so a placeholder would boot the
    // *real* provider holding a credential DocuSign will reject — a green
    // deploy that fails on the first signature request. Requiring the keys
    // unconditionally left a placeholder as the only way to ship, which is why
    // this is a trigger rather than five plain requirements.
    //
    // Declaring the integration still demands every key the real provider
    // reads, so a half-configured DocuSign remains a hard failure. Both
    // deployments set `DOCUSIGN_BASE_URL`, so nothing relaxes for them.
    required!(integration "DOCUSIGN_ACCOUNT_ID" if "DOCUSIGN_BASE_URL"),
    Requirement {
        any_of: &[
            &[
                "DOCUSIGN_INTEGRATION_KEY",
                "DOCUSIGN_USER_ID",
                "DOCUSIGN_PRIVATE_KEY",
            ],
            &["DOCUSIGN_ACCESS_TOKEN"],
        ],
        trigger: Some("DOCUSIGN_BASE_URL"),
        integration: true,
        project_id: None,
    },
    required!(integration "DOCUSIGN_HMAC_KEY" if "DOCUSIGN_BASE_URL"),
    required!(integration "DOCUSIGN_WEBHOOK_SECRET" if "DOCUSIGN_BASE_URL"),
    // The two remaining keys the real provider reads, modelled under the same
    // trigger so the integration is described in one place rather than
    // half here and half in a provider-side check.
    //
    // `DOCUSIGN_OAUTH_BASE` was already enforced for a production JWT
    // credential by `portal::config::enforce_deployment_invariants`, which
    // rejects an empty value once `DOCUSIGN_INTEGRATION_KEY` is set. Stating it
    // here makes that conditional readable from the table every other gate
    // reads. `DOCUSIGN_SIGNER_EMAIL` has no such backstop: `portal` defaults it
    // to a hard-coded address, so a deployment that declares DocuSign and omits
    // it sends envelopes from whatever that default happens to be. A declared
    // integration names its own signer.
    required!(integration "DOCUSIGN_OAUTH_BASE" if "DOCUSIGN_BASE_URL"),
    required!(integration "DOCUSIGN_SIGNER_EMAIL" if "DOCUSIGN_BASE_URL"),
    // The GitHub App behind the webhook receiver and the DevX services: the
    // org plus the App creds that authenticate the server-side
    // JWT→installation-token exchange. All three are needed together;
    // integration-tier, so the staging CI harness may skip them.
    Requirement {
        any_of: &[&[
            "NAVIGATOR_GITHUB_ORG",
            "NAVIGATOR_GITHUB_APP_ID",
            "NAVIGATOR_GITHUB_APP_PRIVATE_KEY",
        ]],
        trigger: None,
        integration: true,
        project_id: None,
    },
    // The GitHub webhook receiver, hosted by `workflows-service` on the public
    // workflows host (`www` goes behind the tailnet): the HMAC secret, the
    // watched product code repo, the App bot login (echo suppression), and the
    // Restate ingress endpoint + bearer the receiver submits through. Still
    // project-scoped to the automation home and preflighted here so `ops ship`
    // refuses an automation-home deployment whose shared secret omits them —
    // the worker reads them from the same `navigator-web-secrets`.
    // `NAVIGATOR_GITHUB_ORG` (the Project-repo org watched by owner) is already
    // required above. Integration-tier: the staging CI harness runs no receiver.
    Requirement {
        any_of: &[&[
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
            "NAVIGATOR_GITHUB_APP_LOGIN",
            "RESTATE_INGRESS_URL",
            "RESTATE_AUTH_TOKEN",
        ]],
        trigger: None,
        integration: true,
        project_id: Some(GITHUB_AUTOMATION_HOME_PROJECT),
    },
    required!(integration "NAVIGATOR_CREDENTIAL_ENVIRONMENT"),
    // Target-aware Slack Web API delivery for per-Project private channels.
    // The bot token is required for both staging and production once this
    // feature is enabled; local/KIND uses the capturing backend when absent.
    required!(integration "SLACK_BOT_TOKEN"),
    required!("SESSION_SECRET"),
    Requirement {
        any_of: &[&["OIDC_AUDIENCE"]],
        trigger: Some("OIDC_JWKS_URL"),
        integration: false,
        project_id: None,
    },
    Requirement {
        any_of: &[&["OIDC_ISSUER"]],
        trigger: Some("OIDC_JWKS_URL"),
        integration: false,
        project_id: None,
    },
    // The OAuth client allowlist Gemini Enterprise's tokens are validated
    // against. Unset, `portal::google_oauth` degrades to a pass-through, so
    // `portal::mcp_principal` never injects a `Principal` — and every AIDA tool
    // that scopes on the authenticated actor (`aida_create_notation`'s
    // project ACL) silently stops checking. Integration-tier: the explicit
    // staging CI harness has no real Google tenant to validate against.
    required!(integration "GOOGLE_OAUTH_CLIENT_IDS"),
    // Sign in with Microsoft, declared by `OAUTH_MICROSOFT_CLIENT_ID` and
    // otherwise absent — the same shape as DocuSign above.
    // `portal::oauth::Provider::microsoft_from_env` reads an unset client id
    // as "no second provider" (`Ok(None)`), so a deployment that has not
    // registered an Entra app supplies neither key. But once the id is set,
    // the client secret is not optional: `microsoft_from_env` returns
    // `OAuthSetupError::Missing` without it, crash-looping the pod on every
    // boot. `OAUTH_MICROSOFT_CLIENT_ID` itself rides inline Deployment env
    // (`ship::INLINE_ENV_WEB_KEYS`), not the Secret rail, so it is not a
    // requirement in its own right here — only the trigger for its secret.
    required!(integration "OAUTH_MICROSOFT_CLIENT_SECRET" if "OAUTH_MICROSOFT_CLIENT_ID"),
    // The SurrealDB coordinates. `web` fails closed on a missing endpoint —
    // `portal::hosting` calls `store::surreal::connect_from_env` with no
    // fallback, and the person directory lives in that engine, so a
    // deployment without one does not degrade: it cannot authenticate anyone.
    //
    // These are coordinates, not key material, so each deployment supplies
    // them from its plaintext `config.toml` rather than the Secret rail. They
    // are listed here so `ops ship` refuses a deployment that is missing one,
    // instead of the absence surfacing as a crash-looping pod — which is what
    // happened while `web` required the handle and this list did not mention
    // it. The root credentials are a separate entry and land with the database
    // user (ENG-18).
    required!("NAVIGATOR_SURREAL_ENDPOINT"),
    required!("NAVIGATOR_SURREAL_NAMESPACE"),
    required!("NAVIGATOR_SURREAL_DATABASE"),
    // The sign-in credentials. A managed engine rejects an anonymous
    // connection, so these are as fatal as the endpoint — and the
    // failure is worse, because `connect` succeeds and the first query
    // is what fails. Required as a pair: half a login is not a login.
    // `NAVIGATOR_SURREAL_AUTH_SCOPE` is deliberately absent — it
    // defaults to `root`, and a deployment naming a different scope is
    // choosing, not satisfying an invariant.
    Requirement {
        any_of: &[&["NAVIGATOR_SURREAL_USER", "NAVIGATOR_SURREAL_PASSWORD"]],
        trigger: None,
        integration: false,
        project_id: None,
    },
];

#[must_use]
pub fn ci_harness_enabled<F: Fn(&str) -> Option<String>>(get: &F) -> bool {
    get(CI_HARNESS).as_deref() == Some("1")
}

/// Whether `NAVIGATOR_CI_HARNESS=1` may relax anything for this profile.
///
/// The flag authorizes in-process fakes for the automated dev harness and
/// nothing beyond it. `k8s/base/web/web.yaml` only ever pairs it with
/// `NAVIGATOR_ENVIRONMENT=dev`, and `docs/third-party-integrations.md` states
/// that production rejects it outright.
///
/// The scoping lives here, in one predicate, rather than at each call site.
/// A caller that tests the raw flag instead grants production the same
/// relaxation the dev harness gets — for a credential-shaped check that means
/// a deployment keeps booting green while pointed at a provider's test
/// environment, which is exactly what these checks exist to prevent.
#[must_use]
pub fn harness_relaxations_apply<F: Fn(&str) -> Option<String>>(
    environment: DeploymentEnvironment,
    get: &F,
) -> bool {
    environment == DeploymentEnvironment::Dev && ci_harness_enabled(get)
}

#[must_use]
pub fn applicable_web_requirements<F: Fn(&str) -> Option<String>>(
    environment: DeploymentEnvironment,
    get: &F,
) -> Vec<Requirement> {
    let harness = harness_relaxations_apply(environment, get);
    let project_id = get("NAVIGATOR_GCP_PROJECT_ID");
    WEB_REQUIREMENTS
        .iter()
        .filter(|requirement| !harness || !requirement.integration)
        .filter(|requirement| {
            requirement
                .project_id
                .is_none_or(|required| project_id.as_deref() == Some(required))
        })
        .filter(|requirement| {
            requirement
                .trigger
                .is_none_or(|key| get(key).is_some_and(|value| !value.is_empty()))
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{applicable_web_requirements, DeploymentEnvironment};

    /// Every key any alternative of any applicable requirement names.
    fn demanded(pairs: &[(&str, &str)]) -> Vec<String> {
        let get = |key: &str| -> Option<String> {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, value)| (*value).to_owned())
        };
        applicable_web_requirements(DeploymentEnvironment::Production, &get)
            .iter()
            .flat_map(|requirement| requirement.any_of.iter().copied())
            .flatten()
            .map(|key| (*key).to_owned())
            .collect()
    }

    const DOCUSIGN_KEYS: &[&str] = &[
        "DOCUSIGN_ACCOUNT_ID",
        "DOCUSIGN_INTEGRATION_KEY",
        "DOCUSIGN_USER_ID",
        "DOCUSIGN_PRIVATE_KEY",
        "DOCUSIGN_ACCESS_TOKEN",
        "DOCUSIGN_HMAC_KEY",
        "DOCUSIGN_WEBHOOK_SECRET",
    ];

    /// A deployment that signs nothing must be able to say so. Without this,
    /// `ops ship` demands DocuSign credentials, and the placeholder an
    /// operator would reach for is worse than the absence:
    /// `DocuSignSignatureProvider::from_env` returns `Some` for any non-empty
    /// value, so the *real* provider boots holding a fake credential and the
    /// first signature request fails at runtime. `StubSignatureProvider` only
    /// engages when DocuSign is genuinely unset.
    #[test]
    fn a_deployment_that_declares_no_docusign_is_asked_for_none() {
        let demanded = demanded(&[("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg")]);
        for key in DOCUSIGN_KEYS {
            assert!(
                !demanded.contains(&(*key).to_owned()),
                "{key} must not be required of a deployment that declares no DOCUSIGN_BASE_URL"
            );
        }
    }

    /// The other half, and the reason this is a trigger rather than a
    /// deletion: declaring DocuSign still demands everything the real
    /// provider needs. A half-configured integration stays a hard failure.
    #[test]
    fn declaring_docusign_demands_every_key_the_real_provider_reads() {
        let demanded = demanded(&[
            ("NAVIGATOR_GCP_PROJECT_ID", "neon-law"),
            ("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"),
        ]);
        for key in DOCUSIGN_KEYS {
            assert!(
                demanded.contains(&(*key).to_owned()),
                "{key} must be required once a deployment declares DOCUSIGN_BASE_URL"
            );
        }
    }

    /// A deployment that has not registered an Entra app must be able to say
    /// so without being asked for a client secret it has no reason to hold.
    #[test]
    fn a_deployment_that_declares_no_microsoft_oauth_is_asked_for_none() {
        let demanded = demanded(&[("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg")]);
        assert!(
            !demanded.contains(&"OAUTH_MICROSOFT_CLIENT_SECRET".to_owned()),
            "OAUTH_MICROSOFT_CLIENT_SECRET must not be required of a deployment that declares no \
             OAUTH_MICROSOFT_CLIENT_ID"
        );
    }

    /// The other half, and the reason this is a trigger rather than a plain
    /// requirement: once a deployment sets `OAUTH_MICROSOFT_CLIENT_ID`,
    /// `portal::oauth::Provider::microsoft_from_env` fails without the
    /// matching secret — the staging crash loop this guards, where the
    /// SecretProviderClass projected the id's inline env but never the
    /// secret.
    #[test]
    fn declaring_microsoft_oauth_demands_the_client_secret() {
        let demanded = demanded(&[
            ("NAVIGATOR_GCP_PROJECT_ID", "neon-law-stg"),
            ("OAUTH_MICROSOFT_CLIENT_ID", "some-entra-app-id"),
        ]);
        assert!(
            demanded.contains(&"OAUTH_MICROSOFT_CLIENT_SECRET".to_owned()),
            "OAUTH_MICROSOFT_CLIENT_SECRET must be required once a deployment declares \
             OAUTH_MICROSOFT_CLIENT_ID"
        );
    }
}
