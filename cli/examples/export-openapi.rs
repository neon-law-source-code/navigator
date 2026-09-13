#![allow(clippy::doc_markdown)]
//! Export the OpenAPI document for a consumer to pin.
//!
//! `navigator-ux` generates its API client from the same document this
//! repository serves at `/app/api/openapi.json`. Rather than keeping a
//! hand-copied second copy of it, it vendors the artifact this example writes,
//! pinned to one immutable Navigator revision. The export is deterministic:
//! the same document at the same revision produces the same bytes, so a
//! consumer that re-runs it can tell "nothing changed" from "somebody edited
//! the generated file".
//!
//! This example pins a document; it does not check one.
//! `server/tests/openapi_drift.rs` is what keeps the source document honest,
//! comparing [`portal::openapi::documented_operations`] against the routes
//! [`portal::api::routes`] registers at `(method, path)` granularity. The
//! export is downstream of that guard and inherits it.
//!
//! This is an example rather than a `navigator` subcommand deliberately. It is
//! a build-time producer step, not something an operator runs against a
//! deployment, and keeping it out of the dispatcher keeps the shipped binary's
//! surface unchanged.
//!
//! ```text
//! cargo run -p cli --example export-openapi -- \
//!     --revision "$(git rev-parse origin/main)" \
//!     --out ../navigator-ux/spec/openapi.json
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use clap::Parser;
use sha2::{Digest, Sha256};

/// The repository the document is authored in, recorded in every export.
const DEFAULT_REPOSITORY: &str = "neon-law-source-code/navigator";

/// The module the document is authored in. Fixed rather than a flag: the
/// exporter serializes compiled-in Rust, so there is no other file it could
/// have read.
const DOCUMENT_SOURCE: &str = "portal/src/openapi.rs";

/// The host an exported document names in `servers` and `contact`.
///
/// [`portal::openapi::document`] resolves that host from the mounted brand,
/// `NAV_BASE_URL`, or the request's `Host` header, which is right for a served
/// document and wrong for a vendored one: a developer who sourced `.devx/env`
/// would publish their worktree's loopback port. Pinning the documentation
/// placeholder keeps the artifact a property of the revision alone. An
/// operator exporting for one deployment overrides it with `--base-url`.
const DEFAULT_BASE_URL: &str = "https://www.your-domain.example";

#[derive(Parser, Debug)]
#[command(
    name = "export-openapi",
    about = "Export the OpenAPI document as a pinned, integrity-checked JSON artifact"
)]
struct Args {
    /// The immutable commit the document is read at. A consumer pins this, so
    /// it must be reachable from `origin/main`, and it must be the commit
    /// actually checked out here with no uncommitted changes — the exporter
    /// serializes whatever source is on disk, not whatever `--revision` says.
    #[arg(long)]
    revision: String,
    /// The producing repository.
    #[arg(long, default_value = DEFAULT_REPOSITORY)]
    repository: String,
    /// The host the exported document names in `servers` and `contact`.
    #[arg(long, default_value = DEFAULT_BASE_URL)]
    base_url: String,
    /// Where to write the artifact. Omitted writes to standard output.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let revision = args.revision.trim();
    if revision.is_empty() {
        bail!("--revision must name the commit the document was read at");
    }
    // A branch name is not a pin. A consumer that recorded `main` would have no
    // way to say which surface it generated against.
    if revision.len() != 40 || !revision.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("--revision must be a full 40-character commit sha, not `{revision}`");
    }
    require_checkout_matches_revision(revision)?;
    require_reachable_from_origin_main(revision)?;

    let source = serde_json::json!({
        "path": DOCUMENT_SOURCE,
        "repository": args.repository,
        "revision": revision,
    });
    let document = export_document(
        &portal::openapi::document_with_base(&args.base_url),
        &source,
    );

    match args.out {
        Some(path) => {
            // A bare filename's parent is `Some("")` rather than `None`, but
            // `std::fs::create_dir_all` special-cases an empty path to a
            // no-op `Ok(())` (see `DirBuilder::create_dir_all` in
            // `library/std/src/fs.rs`), so `--out openapi.json` already
            // writes into the current directory without any extra handling
            // here.
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::write(&path, &document)
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!("wrote {} ({} bytes)", path.display(), document.len());
        }
        None => print!("{document}"),
    }
    Ok(())
}

/// Refuse a `--revision` that does not describe the source actually on disk.
///
/// The reachability check below proves the *label* is honest about surviving
/// a squash merge; it says nothing about whether the bytes this process is
/// about to serialize came from that commit. `document_with_base` compiles
/// whatever `portal/src/openapi.rs` is on disk right now, so a stale checkout,
/// a different (even if also-reachable) commit, or uncommitted local edits
/// would all export current source under someone else's name. Requiring HEAD
/// to equal `--revision` exactly, with a clean working tree, is what makes the
/// recorded revision describe the artifact rather than merely accompany it.
fn require_checkout_matches_revision(revision: &str) -> Result<()> {
    let head = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .context("run `git rev-parse HEAD`")?;
    if !head.status.success() {
        bail!("cannot resolve `HEAD` here: run this from a Navigator git checkout");
    }
    let head = String::from_utf8(head.stdout)
        .context("`git rev-parse HEAD` printed non-UTF-8")?
        .trim()
        .to_string();
    if !head.eq_ignore_ascii_case(revision) {
        bail!(
            "the checked-out HEAD ({head}) does not match --revision {revision}. This exporter \
             serializes whatever source is on disk, not whatever the flag names, so the two must \
             agree or the recorded provenance would be false. Check out {revision} before \
             exporting."
        );
    }

    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .context("run `git status --porcelain`")?;
    if !status.status.success() {
        bail!("cannot run `git status` here: run this from a Navigator git checkout");
    }
    if !status.stdout.is_empty() {
        bail!(
            "the working tree has uncommitted changes, so the exported document may not match \
             --revision {revision}. Commit or discard them before exporting."
        );
    }
    Ok(())
}

/// Refuse a revision the shipped branch does not reach.
///
/// A pull request squash-merges into a *new* commit, and its head survives on
/// no branch afterwards — a fresh clone cannot resolve it. `git rev-parse HEAD`
/// on the branch you exported from is therefore the natural value to type and
/// exactly the one that rots, so shape alone is not enough: the pin has to be
/// an ancestor of `origin/main`.
fn require_reachable_from_origin_main(revision: &str) -> Result<()> {
    let resolved = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "origin/main^{commit}"])
        .output()
        .context("run `git rev-parse origin/main`")?;
    if !resolved.status.success() {
        bail!(
            "cannot resolve `origin/main` here: run `git fetch origin main` from the Navigator \
             checkout before exporting, so the revision you pin can be checked against the \
             shipped branch"
        );
    }
    let tip = String::from_utf8(resolved.stdout)
        .context("`git rev-parse origin/main` printed non-UTF-8")?
        .trim()
        .to_string();

    // An object this repository has never held fails here too, and the message
    // is still true of it: what a consumer needs is reachability, not mere
    // local existence.
    let reachable = Command::new("git")
        .args(["merge-base", "--is-ancestor", revision, &tip])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run `git merge-base --is-ancestor`")?;
    if !reachable.success() {
        bail!(
            "--revision {revision} is not reachable from origin/main. A pull request squash-merges \
             into a new commit, so its head survives on no branch and a fresh clone cannot resolve \
             it. Export after the change lands, pinning `git rev-parse origin/main`."
        );
    }
    Ok(())
}

/// The artifact: the canonical payload, plus the digest that covers it.
///
/// `payload.document` is a whole OpenAPI document a consumer lifts out
/// verbatim; `payload.source` says which revision produced it. Both sit under
/// the payload so one digest covers the document *and* its provenance — a
/// re-pinned artifact is as detectable as an edited one.
///
/// The consumer re-serializes `payload` canonically — compact JSON with every
/// object key sorted — and checks the digest. That is what makes a hand-edited
/// vendored file fail rather than quietly generate a client for a surface
/// nobody serves.
fn export_document(document: &serde_json::Value, source: &serde_json::Value) -> String {
    let payload = serde_json::json!({ "document": document, "source": source });
    let canonical =
        serde_json::to_string(&payload).expect("invariant: the OpenAPI document is plain JSON");
    let digest = format!("sha256:{}", hex(&Sha256::digest(canonical.as_bytes())));
    let artifact = serde_json::json!({
        "generator": "cargo run -p cli --example export-openapi",
        "integrity": digest,
        "payload": payload,
    });
    let mut out = serde_json::to_string_pretty(&artifact)
        .expect("invariant: the export document is plain JSON data");
    out.push('\n');
    out
}

/// Lower-case hex, the form both the Rust exporter and the TypeScript consumer
/// write a SHA-256 digest in.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}
