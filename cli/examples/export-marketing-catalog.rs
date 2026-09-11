//! Export the shared marketing catalog for a consumer to pin.
//!
//! `navigator-ux` publishes the same sentences this repository does. Rather
//! than keeping a second copy of them, it vendors the artifact this example
//! writes, pinned to one immutable Navigator revision. The export is
//! deterministic: the same catalog at the same revision produces the same
//! bytes, so a consumer that re-runs it can tell "nothing changed" from
//! "somebody edited the generated file".
//!
//! This is an example rather than a `navigator` subcommand deliberately. It
//! is a build-time producer step, not something an operator runs against a
//! deployment, and keeping it out of the dispatcher keeps the shipped binary's
//! surface unchanged.
//!
//! ```text
//! cargo run -p cli --example export-marketing-catalog -- \
//!     --catalog neon/locales/en/shared.yaml \
//!     --revision "$(git rev-parse HEAD)" \
//!     --out ../navigator-ux/gallery/content/marketing-catalog.json
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use sha2::{Digest, Sha256};
use views::locales::shared::{Provenance, SharedCatalog};

/// The repository a catalog is authored in, recorded in every export.
const DEFAULT_REPOSITORY: &str = "neon-law-source-code/navigator";

#[derive(Parser, Debug)]
#[command(
    name = "export-marketing-catalog",
    about = "Export the shared marketing catalog as a pinned, integrity-checked JSON artifact"
)]
struct Args {
    /// The shared catalog to export.
    #[arg(long, default_value = "neon/locales/en/shared.yaml")]
    catalog: PathBuf,
    /// The immutable commit the catalog is read at. A consumer pins this.
    #[arg(long)]
    revision: String,
    /// The producing repository.
    #[arg(long, default_value = DEFAULT_REPOSITORY)]
    repository: String,
    /// Where to write the artifact. Omitted writes to standard output.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let revision = args.revision.trim();
    if revision.is_empty() {
        bail!("--revision must name the commit the catalog was read at");
    }
    // A branch name is not a pin. A consumer that recorded `main` would have
    // no way to say which words it was built against.
    if revision.len() != 40 || !revision.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("--revision must be a full 40-character commit sha, not `{revision}`");
    }

    let raw = std::fs::read_to_string(&args.catalog)
        .with_context(|| format!("read {}", args.catalog.display()))?;
    let catalog = SharedCatalog::parse(&raw).map_err(anyhow::Error::msg)?;

    let source = Provenance {
        path: args.catalog.to_string_lossy().replace('\\', "/"),
        repository: args.repository.clone(),
        revision: revision.to_string(),
    };
    let document = export_document(&catalog, &source);

    match args.out {
        Some(path) => {
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

/// The artifact: the canonical payload, plus the digest that covers it.
///
/// The consumer re-serializes `payload` canonically — compact JSON with every
/// object key sorted — and checks the digest. That is what makes a hand-edited
/// vendored file fail rather than quietly render different words.
fn export_document(catalog: &SharedCatalog, source: &Provenance) -> String {
    let payload = catalog.canonical_payload(source);
    let digest = format!("sha256:{}", hex(&Sha256::digest(payload.as_bytes())));
    let value: serde_json::Value =
        serde_json::from_str(&payload).expect("invariant: the canonical payload is JSON");
    let document = serde_json::json!({
        "generator": "cargo run -p cli --example export-marketing-catalog",
        "integrity": digest,
        "payload": value,
    });
    let mut out = serde_json::to_string_pretty(&document)
        .expect("invariant: the export document is plain JSON data");
    out.push('\n');
    out
}

/// Lower-case hex, the form both the Rust exporter and the TypeScript
/// consumer write a SHA-256 digest in.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}
