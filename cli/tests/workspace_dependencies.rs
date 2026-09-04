//! The workspace's dependency set, stated in full.
//!
//! Every third-party crate the workspace builds against is named in the root
//! `[workspace.dependencies]` table, and every member draws from that table
//! with `workspace = true`. Those two facts together make this file the whole
//! description of what enters the build graph: a crate reaches a member only
//! by appearing in [`WORKSPACE_DEPENDENCIES`], and a member-local version is
//! confined to [`MEMBER_LOCAL_DEPENDENCIES`].
//!
//! Stating the set is what makes an addition visible. A new member is written
//! by copying an existing `Cargo.toml`, which carries whatever the file it was
//! copied from named; without an enumerated set, an inherited dependency
//! compiles, passes, and joins the build graph with nothing to report it. Here
//! it is a diff against a list a reviewer reads.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

/// Every crate in the root `[workspace.dependencies]` table. Workspace
/// members appear alongside third-party crates because members depend on each
/// other through the same table.
const WORKSPACE_DEPENDENCIES: &[&str] = &[
    "anyhow",
    "archives",
    "arrow",
    "assert_cmd",
    "async-trait",
    "aws-credential-types",
    "aws-sdk-s3",
    "axum",
    "base64",
    "billing",
    "billing-workflows",
    "bs58",
    "bytes",
    "chrono",
    "clap",
    "cloud",
    "comfy-table",
    "cucumber",
    "dioxus",
    "dioxus-core",
    "dioxus-fullstack-core",
    "dioxus-server",
    "dotenvy",
    "ed25519-dalek",
    "encoding_rs",
    "fantoccini",
    "features",
    "flate2",
    "forms",
    "futures",
    "gateway",
    "github-runner",
    "github_webhooks",
    "google-cloud-auth",
    "google-cloud-storage",
    "google-cloud-token",
    "hmac",
    "http-body-util",
    "image",
    "import",
    "include_dir",
    "ipnet",
    "jsonwebtoken",
    "k8s-openapi",
    "kube",
    "live-inquiry",
    "lopdf",
    "mail-parser",
    "mcp",
    "neon",
    "opentelemetry",
    "opentelemetry-appender-tracing",
    "opentelemetry-otlp",
    "opentelemetry_sdk",
    "owo-colors",
    "p256",
    "parquet",
    "pdf",
    "percent-encoding",
    "pingora",
    "portal",
    "predicates",
    "pulldown-cmark",
    "rand",
    "ravif",
    "regorus",
    "repos",
    "reqwest",
    "restate-sdk",
    "rgb",
    "rules",
    "scraper",
    "semver",
    "serde",
    "serde_json",
    "serde_yaml",
    "server",
    "sha2",
    "store",
    "strum",
    "surrealdb",
    "symphonia",
    "syntect",
    "telemetry",
    "tempfile",
    "thiserror",
    "tokio",
    "toml",
    "tower",
    "tower-cookies",
    "tower-http",
    "tracing",
    "tracing-core",
    "tracing-opentelemetry",
    "tracing-subscriber",
    "url",
    "uuid",
    "views",
    "walkdir",
    "webapp",
    "webp",
    "wiremock",
    "workflows",
    "zip",
];

/// Dependencies a member pins itself instead of inheriting, as
/// `(member, crate)`. Each has exactly one consumer, so its version lives with
/// the crate that uses it rather than in a table every member reads.
///
/// A second consumer is the signal to promote the crate into
/// [`WORKSPACE_DEPENDENCIES`], which is what keeps two members from building
/// against two versions of the same library.
const MEMBER_LOCAL_DEPENDENCIES: &[(&str, &str)] = &[
    // `store` selects the `aws-lc-rs` provider explicitly. A `wss://` Surreal
    // endpoint needs a registered rustls `CryptoProvider`, and the choice of
    // backend belongs with the crate that opens the connection.
    ("store", "rustls"),
    // The Dioxus server-side renderer, used by the crate that renders.
    ("webapp", "dioxus-ssr"),
    // Assertions over rendered markup in the `views` test suite.
    ("views", "regex"),
    // The Language Server Protocol wire types.
    ("lsp", "lsp-types"),
    // Typst compiles the generated PDFs, and `pdf` is the crate that runs it.
    ("pdf", "typst-as-lib"),
    ("pdf", "typst-layout"),
    ("pdf", "typst-pdf"),
    // The Iceberg table format the archive lane writes.
    ("archives", "iceberg"),
];

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn manifest(path: &Path) -> Table {
    let body =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    body.parse::<Table>()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// The member directory names listed in the root manifest's `workspace.members`.
fn members(root_manifest: &Table) -> Vec<String> {
    root_manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(Value::as_array)
        .expect("the root manifest declares workspace.members")
        .iter()
        .map(|member| {
            member
                .as_str()
                .expect("every workspace member is a string")
                .to_string()
        })
        .collect()
}

/// The three dependency tables a member manifest may declare.
const DEPENDENCY_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

#[test]
fn the_workspace_dependency_table_is_the_declared_set() {
    let root = manifest(&repo_root().join("Cargo.toml"));
    let declared: BTreeSet<String> = root
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(Value::as_table)
        .expect("the root manifest declares workspace.dependencies")
        .keys()
        .cloned()
        .collect();
    let expected: BTreeSet<String> = WORKSPACE_DEPENDENCIES
        .iter()
        .map(ToString::to_string)
        .collect();

    let added: Vec<&String> = declared.difference(&expected).collect();
    let removed: Vec<&String> = expected.difference(&declared).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "the workspace dependency set changed. In Cargo.toml but not in \
         WORKSPACE_DEPENDENCIES: {added:?}. In WORKSPACE_DEPENDENCIES but not in Cargo.toml: \
         {removed:?}. Every crate the workspace builds against is named in that list — update it \
         in the same commit that changes the table.",
    );
}

#[test]
fn every_member_draws_its_dependencies_from_the_workspace_table() {
    let root_manifest = manifest(&repo_root().join("Cargo.toml"));
    let allowed: BTreeSet<&str> = WORKSPACE_DEPENDENCIES.iter().copied().collect();
    let mut offenders = Vec::new();
    let mut seen_members = 0;

    for member in members(&root_manifest) {
        let path = repo_root().join(&member).join("Cargo.toml");
        assert!(
            path.is_file(),
            "workspace.members names `{member}`, which has no Cargo.toml — a guard pointed at a \
             moved member passes for the wrong reason",
        );
        seen_members += 1;
        let member_manifest = manifest(&path);

        for table in DEPENDENCY_TABLES {
            let Some(entries) = member_manifest.get(*table).and_then(Value::as_table) else {
                continue;
            };
            for (name, spec) in entries {
                // Inherited from the root table: already covered by
                // `the_workspace_dependency_table_is_the_declared_set`.
                if spec.get("workspace").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                // A path dependency resolves inside this repository and adds
                // nothing third-party to the graph.
                if spec.get("path").is_some() {
                    continue;
                }
                if MEMBER_LOCAL_DEPENDENCIES.contains(&(member.as_str(), name.as_str())) {
                    continue;
                }
                let inherited = if allowed.contains(name.as_str()) {
                    " (it is in the workspace table — use `workspace = true`)"
                } else {
                    ""
                };
                offenders.push(format!("{member}/Cargo.toml [{table}] {name}{inherited}"));
            }
        }
    }

    assert!(
        seen_members >= 25,
        "expected the full member list and read {seen_members} — this guard has stopped reading \
         the manifests it is supposed to read",
    );
    assert!(
        offenders.is_empty(),
        "these dependencies enter the build graph without passing through the workspace table. \
         Add the crate to [workspace.dependencies] and inherit it with `workspace = true`, or \
         record a deliberate exception in MEMBER_LOCAL_DEPENDENCIES:\n{}",
        offenders.join("\n"),
    );
}
