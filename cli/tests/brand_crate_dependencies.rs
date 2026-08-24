//! The thin-brand-crate rule, enforced.
//!
//! The two-binary host pair that #860 removed rotted into two 95%-identical
//! stacks: each grew its own reach into the domain crates until neither could
//! be changed without the other. The replacement (#974) is a crate per brand,
//! each *composition only* — a `portal::hosting::Brand` value and a call to
//! the shared run loop — so there is nothing in them to drift.
//!
//! Nothing but a test keeps them that way. A brand crate may name the
//! application crate it mounts, `views` (brand and layout), `webapp` (the page
//! components it renders), and `telemetry`. Reaching past those for `store`,
//! `workflows`, or the auth machinery is how a brand starts making decisions
//! the application is supposed to own — a public page that needs a domain read
//! goes through what the application crate deliberately re-exports instead.
//!
//! A brand crate is no longer only a `Brand` value: each now owns its own
//! public face — copy, page compositions, path table — because that surface is
//! the one thing a brand crate does not share. The rule that matters is
//! therefore about the *domain*, not about line count.
//!
//! Brand crates opt in with `[package.metadata.navigator] brand = true`, so a
//! brand added later is governed the day it lands rather than the day someone
//! remembers to extend a list in this file.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Workspace crates a brand crate may depend on.
///
/// `portal` is the application crate — the thing a brand mounts, and the only
/// legitimate route to a domain read. `views` carries brand identity and
/// layout, `webapp` the page components a public surface renders, and
/// `telemetry` is the observability seam. Everything else in the workspace is
/// the application's business, not a brand's.
///
/// The line this draws is *domain*, not size. A brand owns its public face —
/// its copy, its page compositions, its path table — because that is the one
/// thing brands genuinely do not share. What it may never name is
/// `store`, `workflows`, `billing`, `rules`, or the auth machinery: those
/// decide what is true and who may see it, and a brand that reached them
/// could fork authorization from the application it claims to mount.
const ALLOWED_WORKSPACE_DEPENDENCIES: &[&str] = &["portal", "views", "webapp", "telemetry"];

/// The workspace root (this test crate is `cli`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

fn read_manifest(path: &Path) -> toml::Table {
    let body = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&body).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Every `[workspace] members` entry, in declaration order.
fn workspace_members(root: &Path) -> Vec<String> {
    read_manifest(&root.join("Cargo.toml"))
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
        .expect("[workspace] members")
        .iter()
        .map(|m| m.as_str().expect("workspace member is a string").to_owned())
        .collect()
}

/// True when the manifest declares `[package.metadata.navigator] brand = true`.
fn is_brand_crate(manifest: &toml::Table) -> bool {
    manifest
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("navigator"))
        .and_then(|n| n.get("brand"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// The names in a manifest's `[dependencies]` table.
fn dependency_names(manifest: &toml::Table) -> BTreeSet<String> {
    manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn brand_crates_depend_only_on_the_application_its_views_and_telemetry() {
    let root = workspace_root();
    let members = workspace_members(&root);
    let workspace_crates: BTreeSet<&str> = members.iter().map(String::as_str).collect();

    let mut checked = Vec::new();
    for member in &members {
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest = read_manifest(&manifest_path);
        if !is_brand_crate(&manifest) {
            continue;
        }
        checked.push(member.clone());

        // Only `[dependencies]`, and within it only workspace crates. A brand
        // `main` still needs `anyhow` and `tokio` to have a signature and a
        // runtime, and a brand's own tests may reach for fixtures — neither
        // ships in the binary. What the rule is about is what the deployed
        // brand links: reaching into the *domain*.
        let offenders: Vec<String> = dependency_names(&manifest)
            .into_iter()
            .filter(|dep| workspace_crates.contains(dep.as_str()))
            .filter(|dep| !ALLOWED_WORKSPACE_DEPENDENCIES.contains(&dep.as_str()))
            .collect();

        assert!(
            offenders.is_empty(),
            "brand crate `{member}` depends on {offenders:?}; a brand crate is composition only \
             and may name just {ALLOWED_WORKSPACE_DEPENDENCIES:?}. A public page that needs a \
             domain read goes through what the application crate re-exports."
        );
    }

    assert!(
        !checked.is_empty(),
        "no crate declares `[package.metadata.navigator] brand = true`; the marker this rule is \
         keyed on has gone missing, so the rule now enforces nothing"
    );
}

/// The brand crates that must carry the marker: every first-party host
/// #974 defines, less the retired platform brand (#1180).
const KNOWN_BRAND_CRATES: &[&str] = &["neon"];

/// The marker is only load-bearing if it is on the crates that are actually
/// brands. A brand crate that quietly drops it would pass the test above by
/// disappearing from it.
#[test]
fn every_known_brand_crate_carries_the_marker() {
    let root = workspace_root();
    for brand in KNOWN_BRAND_CRATES {
        let manifest_path = root.join(brand).join("Cargo.toml");
        assert!(
            manifest_path.is_file(),
            "{} is missing",
            manifest_path.display()
        );
        assert!(
            is_brand_crate(&read_manifest(&manifest_path)),
            "{brand} must declare `[package.metadata.navigator] brand = true` or it escapes the \
             thin-crate rule"
        );
    }
}
