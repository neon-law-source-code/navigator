//! `navigator ops notices` — regenerate the third-party licence notices that
//! ship with the downloadable `navigator` binary.
//!
//! A statically linked Rust binary carries the compiled form of every crate in
//! its dependency tree, and the permissive licences those crates use — the set
//! `deny.toml` allows — each require their notice to travel with the
//! distributed work. Apache-2.0 section 4 says so explicitly; MIT, ISC, and the
//! BSD family all require the copyright notice to be retained, so this file has
//! to exist and stay current.
//!
//! **Deduplicated by text.** Concatenating 1,300-odd licence files verbatim
//! produces megabytes in which the same Apache-2.0 body appears hundreds of
//! times. Instead every *distinct* licence text is emitted once, listing the
//! crates that carry it. Nothing is summarised or rewritten: each text appears
//! in full, which is what the licences require. Only the repetition goes.
//!
//! **Over-inclusive on purpose.** The crate set comes from `Cargo.lock`, which
//! is a superset of what any one binary links: it includes dev-dependencies and
//! crates for other target platforms. Naming a crate whose code did not ship is
//! harmless; omitting one whose code did is the compliance failure. When the
//! set is later narrowed, narrow it deliberately.
//!
//! **An unpacked source is a precondition, not an outcome.** Licence text is
//! read from `$CARGO_HOME/registry/src`, which cargo populates per crate as it
//! unpacks one — `cargo fetch` unpacks every target platform's graph, while a
//! build unpacks only the platform it built for. A crate whose source is absent
//! therefore says nothing about that crate's licence; it says this machine never
//! unpacked it. Folding that into the no-licence-file list would publish one
//! machine's gap as the crate's, so an absent source refuses the run instead.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// One crate in the dependency tree, as `Cargo.lock` names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Crate {
    pub name: String,
    pub version: String,
}

impl std::fmt::Display for Crate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}

/// Filenames that carry a licence or attribution notice. Matched
/// case-insensitively against the file *stem* so `LICENSE`, `LICENSE-MIT`,
/// `LICENCE.txt`, `COPYING`, and `NOTICE` all land.
fn is_notice_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let named_like_a_notice = ["license", "licence", "copying", "notice"]
        .iter()
        .any(|stem| lower.starts_with(stem));
    // `LICENSE.spdx` and `license.toml` are metadata, not the text. Compared
    // through `Path::extension` on the already-lowercased name so the check is
    // case-insensitive by construction.
    let extension = Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    named_like_a_notice && !matches!(extension, "spdx" | "toml")
}

/// Every registry crate in a `Cargo.lock`, sorted and deduplicated.
///
/// Workspace members have no `source` key and are excluded: they are the
/// Firm's own code, governed by root `LICENSE`, and are not third-party.
pub fn registry_crates(lockfile: &str) -> Vec<Crate> {
    let doc: toml::Value = match toml::from_str(lockfile) {
        Ok(doc) => doc,
        Err(_) => return Vec::new(),
    };
    let Some(packages) = doc.get("package").and_then(|p| p.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<Crate> = packages
        .iter()
        .filter(|pkg| {
            pkg.get("source")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.starts_with("registry+"))
        })
        .filter_map(|pkg| {
            Some(Crate {
                name: pkg.get("name")?.as_str()?.to_string(),
                version: pkg.get("version")?.as_str()?.to_string(),
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `$CARGO_HOME/registry/src/<index>/`, for every index present. A machine that
/// has fetched from more than one registry mirror has more than one.
fn registry_src_roots() -> Vec<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    let Some(src) = cargo_home.map(|h| h.join("registry").join("src")) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&src) else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    roots.sort();
    roots
}

/// What this machine's registry can say about one crate's notices.
///
/// The two cases are not interchangeable, and conflating them is how a notices
/// file under-attributes: `Absent` is a fact about the machine, while
/// `Present(vec![])` is a fact about the crate's published archive.
enum Sources {
    /// No unpacked source directory under any registry root.
    Absent,
    /// The source is unpacked. Carries every notice text found in it, which is
    /// empty for a crate that publishes none.
    Present(Vec<String>),
}

/// The notice texts a single crate's extracted source carries, sorted by
/// filename so the output does not depend on directory iteration order.
fn notices_for(roots: &[PathBuf], krate: &Crate) -> Sources {
    let dir_name = format!("{}-{}", krate.name, krate.version);
    let mut unpacked = false;
    let mut texts = Vec::new();
    for root in roots {
        let dir = root.join(&dir_name);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        unpacked = true;
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_notice_file)
            })
            .collect();
        files.sort();
        for file in files {
            if let Ok(body) = fs::read_to_string(&file) {
                let trimmed = body.trim();
                if !trimmed.is_empty() {
                    texts.push(trimmed.to_string());
                }
            }
        }
        if !texts.is_empty() {
            break;
        }
    }
    if unpacked {
        Sources::Present(texts)
    } else {
        Sources::Absent
    }
}

/// The SPDX expression a crate declares in its own manifest.
///
/// Most crates that ship no licence *file* still declare `license = "MIT OR
/// Apache-2.0"` in `Cargo.toml` — publishing the text is conventional, not
/// required by crates.io. That declaration is the attribution for those crates,
/// and the full text of the licence it names is already in this file from the
/// hundreds of crates that do ship it.
fn declared_license(roots: &[PathBuf], krate: &Crate) -> Option<String> {
    let dir_name = format!("{}-{}", krate.name, krate.version);
    for root in roots {
        let Ok(body) = fs::read_to_string(root.join(&dir_name).join("Cargo.toml")) else {
            continue;
        };
        let Ok(doc) = toml::from_str::<toml::Value>(&body) else {
            continue;
        };
        let Some(package) = doc.get("package") else {
            continue;
        };
        if let Some(spdx) = package.get("license").and_then(|l| l.as_str()) {
            return Some(spdx.to_string());
        }
        if let Some(file) = package.get("license-file").and_then(|l| l.as_str()) {
            return Some(format!("see bundled {file}"));
        }
    }
    None
}

/// A crate carrying no licence file, with whatever its manifest declares.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Undeclared {
    pub krate: Crate,
    pub spdx: Option<String>,
}

/// The grouped notice set: distinct licence text to the crates carrying it,
/// plus the crates that ship no licence file.
pub struct Notices {
    /// Licence text to the crates that carry it. `BTreeMap` keyed by text keeps
    /// the rendering deterministic without a separate sort.
    pub by_text: BTreeMap<String, Vec<Crate>>,
    /// Crates whose published archive contains no licence file.
    pub gaps: Vec<Undeclared>,
    /// Crates with no unpacked source on this machine, so nothing was read for
    /// them at all. No licensing statement can be made from this, which is why
    /// a non-empty set refuses the run rather than rendering.
    pub absent: Vec<Crate>,
}

/// Group every crate's notice texts, collapsing identical texts.
pub fn collect(roots: &[PathBuf], crates: &[Crate]) -> Notices {
    let mut by_text: BTreeMap<String, Vec<Crate>> = BTreeMap::new();
    let mut gaps = Vec::new();
    let mut absent = Vec::new();
    for krate in crates {
        match notices_for(roots, krate) {
            Sources::Absent => absent.push(krate.clone()),
            Sources::Present(texts) if texts.is_empty() => gaps.push(Undeclared {
                krate: krate.clone(),
                spdx: declared_license(roots, krate),
            }),
            Sources::Present(texts) => {
                for text in texts {
                    by_text.entry(text).or_default().push(krate.clone());
                }
            }
        }
    }
    for crates in by_text.values_mut() {
        crates.sort();
        crates.dedup();
    }
    gaps.sort();
    absent.sort();
    Notices {
        by_text,
        gaps,
        absent,
    }
}

/// Render the notices file. Plain text, not Markdown: licence bodies carry
/// their own wrapping and would fail the workspace Markdown line-width rule,
/// and a notices file is conventionally plain text anyway.
pub fn render(notices: &Notices) -> String {
    let mut out = String::new();
    out.push_str(
        "THIRD-PARTY NOTICES\n\
         ===================\n\n\
         The Neon Law Navigator `navigator` binary is copyright Shook Law PLLC and is\n\
         licensed under BUSL-1.1; see LICENSE. It incorporates the third-party\n\
         open-source components listed below, each governed by its own licence, reproduced\n\
         here in full.\n\n\
         Identical licence texts are listed once with every crate that carries them. This file\n\
         is generated by `navigator ops notices` from Cargo.lock; do not edit it by hand.\n\n",
    );

    for (text, crates) in &notices.by_text {
        out.push_str(&"-".repeat(88));
        out.push('\n');
        for krate in crates {
            let _ = writeln!(out, "{krate}");
        }
        out.push('\n');
        out.push_str(text);
        out.push_str("\n\n");
    }

    if !notices.gaps.is_empty() {
        out.push_str(&"-".repeat(88));
        out.push_str(
            "\nThe following crates publish no licence file in their crates.io archive — shipping\n\
             the text is conventional, not required — and declare their licence in the manifest\n\
             instead. Each declared licence is one of the permissive licences allowed by\n\
             deny.toml, and the full text of every one of them appears above, reproduced from\n\
             the crates that do ship it.\n\n",
        );
        for gap in &notices.gaps {
            let _ = match &gap.spdx {
                Some(spdx) => writeln!(out, "{}  —  {spdx}", gap.krate),
                None => writeln!(out, "{}  —  no licence declared", gap.krate),
            };
        }
        out.push('\n');
    }

    // Unreachable through `run`, which refuses before rendering. Rendered
    // anyway so that no caller can quietly produce an under-attributing file:
    // the incompleteness is stated where a reader would look for the licence.
    if !notices.absent.is_empty() {
        out.push_str(&"-".repeat(88));
        out.push_str(
            "\nSOURCE NOT AVAILABLE — this file is incomplete and must not be distributed. The\n\
             crates below have no unpacked source under $CARGO_HOME/registry/src on the machine\n\
             that generated it, so their licences were never read. This says nothing about what\n\
             those crates license under. Run `cargo fetch` and regenerate.\n\n",
        );
        for krate in &notices.absent {
            let _ = writeln!(out, "{krate}");
        }
        out.push('\n');
    }

    out
}

/// What an operator reads when the registry is only partly unpacked.
///
/// A pure function so the message an operator has to act on is covered by a
/// test rather than only by running the command. Names at most ten crates: the
/// remedy is the same for one as for four hundred, and a full list buries it.
fn absent_source_error(absent: &[Crate], total: usize) -> String {
    const SHOWN: usize = 10;
    let mut out = format!(
        "navigator: ops notices: {} of {total} crates in Cargo.lock have no unpacked source \
         under $CARGO_HOME/registry/src, so their licences could not be read. \
         Run `cargo fetch` and try again.\n",
        absent.len()
    );
    for krate in absent.iter().take(SHOWN) {
        let _ = writeln!(out, "  {krate}");
    }
    if absent.len() > SHOWN {
        let _ = writeln!(out, "  … and {} more", absent.len() - SHOWN);
    }
    out
}

/// Entry point for `navigator ops notices`.
pub fn run(out_path: &Path, check: bool) -> ExitCode {
    let lockfile = match fs::read_to_string("Cargo.lock") {
        Ok(body) => body,
        Err(e) => {
            eprintln!("navigator: ops notices: read Cargo.lock: {e}");
            return ExitCode::from(2);
        }
    };
    let crates = registry_crates(&lockfile);
    if crates.is_empty() {
        eprintln!("navigator: ops notices: no registry crates in Cargo.lock");
        return ExitCode::from(2);
    }
    let roots = registry_src_roots();
    if roots.is_empty() {
        eprintln!(
            "navigator: ops notices: no crate sources under $CARGO_HOME/registry/src — \
             run `cargo fetch` first"
        );
        return ExitCode::from(2);
    }
    let notices = collect(&roots, &crates);
    // Refuse before rendering. An absent source is not a licence fact about the
    // crate, so there is no honest way to put it in the file: reading a partial
    // registry and shipping the result is exactly the under-attribution this
    // command exists to prevent. `cargo fetch` unpacks every target's graph.
    if !notices.absent.is_empty() {
        eprint!("{}", absent_source_error(&notices.absent, crates.len()));
        return ExitCode::from(2);
    }
    let rendered = render(&notices);

    if check {
        let current = fs::read_to_string(out_path).unwrap_or_default();
        if current == rendered {
            println!(
                "navigator: ops notices: {} is current ({} crates, {} distinct licence texts)",
                out_path.display(),
                crates.len(),
                notices.by_text.len()
            );
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "navigator: ops notices: {} is stale — re-run `navigator ops notices` and commit it",
            out_path.display()
        );
        return ExitCode::from(1);
    }

    if let Err(e) = fs::write(out_path, &rendered) {
        eprintln!("navigator: ops notices: write {}: {e}", out_path.display());
        return ExitCode::from(2);
    }
    println!(
        "navigator: ops notices: wrote {} — {} crates, {} distinct licence texts, {} gap(s)",
        out_path.display(),
        crates.len(),
        notices.by_text.len(),
        notices.gaps.len()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{
        absent_source_error, collect, is_notice_file, registry_crates, render, Crate, Undeclared,
    };

    #[test]
    fn notice_filenames_match_the_conventional_spellings() {
        for name in [
            "LICENSE",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "licence.txt",
            "COPYING",
            "NOTICE",
        ] {
            assert!(is_notice_file(name), "{name} should be a notice file");
        }
        for name in [
            "src",
            "Cargo.toml",
            "license.toml",
            "LICENSE.spdx",
            "README",
        ] {
            assert!(!is_notice_file(name), "{name} should not be a notice file");
        }
    }

    /// Workspace members carry no `source` key and are the Firm's own code.
    #[test]
    fn only_registry_crates_are_third_party() {
        let lock = r#"
[[package]]
name = "cli"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "local-path-dep"
version = "0.2.0"
source = "git+https://example.invalid/repo"
"#;
        assert_eq!(
            registry_crates(lock),
            vec![Crate {
                name: "serde".into(),
                version: "1.0.0".into()
            }]
        );
    }

    #[test]
    fn malformed_lockfile_yields_no_crates_rather_than_panicking() {
        assert!(registry_crates("this is not toml {{{").is_empty());
    }

    /// The dedup is the whole point: one Apache-2.0 body, every crate listed.
    #[test]
    fn identical_texts_collapse_into_one_entry() {
        let mut notices = collect(&[], &[]);
        let shared = "Apache License, Version 2.0 …".to_string();
        notices.by_text.insert(
            shared,
            vec![
                Crate {
                    name: "aaa".into(),
                    version: "1.0.0".into(),
                },
                Crate {
                    name: "zzz".into(),
                    version: "2.0.0".into(),
                },
            ],
        );

        let out = render(&notices);
        assert_eq!(
            out.matches("Apache License, Version 2.0").count(),
            1,
            "the shared text must appear exactly once"
        );
        assert!(out.contains("aaa 1.0.0"));
        assert!(out.contains("zzz 2.0.0"));
    }

    /// A crate this machine never unpacked is a fact about the machine. Putting
    /// it in the no-licence-file list publishes that gap as the crate's, which
    /// is how a permissive licence's notice silently fails to ship.
    #[test]
    fn a_crate_with_no_unpacked_source_is_absent_not_a_gap() {
        let never_fetched = Crate {
            name: "never-fetched".into(),
            version: "9.9.9".into(),
        };
        let notices = collect(&[], std::slice::from_ref(&never_fetched));
        assert_eq!(notices.absent, vec![never_fetched]);
        assert!(
            notices.gaps.is_empty(),
            "an unread crate must not be reported as publishing no licence file"
        );
    }

    /// Rendered loudly too, so no caller can quietly produce a file that
    /// under-attributes: `run` refuses, and the text refuses with it.
    #[test]
    fn an_absent_source_renders_a_refusal_not_a_licence_claim() {
        let notices = collect(
            &[],
            &[Crate {
                name: "never-fetched".into(),
                version: "9.9.9".into(),
            }],
        );
        let out = render(&notices);
        assert!(out.contains("SOURCE NOT AVAILABLE"));
        assert!(out.contains("never-fetched 9.9.9"));
        assert!(out.contains("cargo fetch"));
        assert!(
            !out.contains("publish no licence file"),
            "an unread crate must not be described as publishing no licence file"
        );
    }

    /// The other half of the split, and the case the gap list actually
    /// describes: the source IS unpacked and ships no notice file, so the
    /// manifest declaration is the attribution.
    #[test]
    fn a_crate_whose_source_ships_no_licence_file_is_a_gap() {
        let root = tempfile::tempdir().expect("tempdir");
        let krate = Crate {
            name: "no-licence-file".into(),
            version: "1.0.0".into(),
        };
        let dir = root.path().join("no-licence-file-1.0.0");
        std::fs::create_dir(&dir).expect("create crate dir");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"no-licence-file\"\nlicense = \"MIT OR Apache-2.0\"\n",
        )
        .expect("write manifest");

        let notices = collect(&[root.path().to_path_buf()], std::slice::from_ref(&krate));
        assert!(
            notices.absent.is_empty(),
            "the source is unpacked, so this is not an absent-source case"
        );
        assert_eq!(
            notices.gaps,
            vec![Undeclared {
                krate,
                spdx: Some("MIT OR Apache-2.0".into())
            }]
        );
        let out = render(&notices);
        assert!(out.contains("publish no licence file"));
        assert!(out.contains("no-licence-file 1.0.0  —  MIT OR Apache-2.0"));
        assert!(!out.contains("SOURCE NOT AVAILABLE"));
    }

    /// Unpacked, no notice file, and no manifest declaration either — the one
    /// genuinely unattributed case, which must still be named.
    #[test]
    fn an_unpacked_crate_declaring_nothing_is_named_as_undeclared() {
        let root = tempfile::tempdir().expect("tempdir");
        let krate = Crate {
            name: "silent".into(),
            version: "0.1.0".into(),
        };
        std::fs::create_dir(root.path().join("silent-0.1.0")).expect("create crate dir");

        let notices = collect(&[root.path().to_path_buf()], std::slice::from_ref(&krate));
        assert!(notices.absent.is_empty());
        assert_eq!(notices.gaps, vec![Undeclared { krate, spdx: None }]);
        assert!(render(&notices).contains("silent 0.1.0  —  no licence declared"));
    }

    /// And the ordinary path: an unpacked crate carrying a notice file is
    /// attributed from its text, in neither exception list.
    #[test]
    fn an_unpacked_crate_carrying_a_licence_file_is_attributed() {
        let root = tempfile::tempdir().expect("tempdir");
        let krate = Crate {
            name: "with-licence".into(),
            version: "2.0.0".into(),
        };
        let dir = root.path().join("with-licence-2.0.0");
        std::fs::create_dir(&dir).expect("create crate dir");
        std::fs::write(
            dir.join("LICENSE-MIT"),
            "MIT License\n\nPermission is hereby granted",
        )
        .expect("write licence");

        let notices = collect(&[root.path().to_path_buf()], std::slice::from_ref(&krate));
        assert!(notices.absent.is_empty());
        assert!(notices.gaps.is_empty());
        let out = render(&notices);
        assert!(out.contains("with-licence 2.0.0"));
        assert!(out.contains("Permission is hereby granted"));
    }

    /// The operator-facing remedy. It must name the count, the remedy, and
    /// enough crates to recognise the shape — without printing four hundred
    /// lines that bury the one instruction that matters.
    #[test]
    fn the_absent_source_error_names_the_count_and_the_remedy() {
        let absent: Vec<Crate> = (0..12)
            .map(|i| Crate {
                name: format!("crate-{i:02}"),
                version: "1.0.0".into(),
            })
            .collect();
        let msg = absent_source_error(&absent, 1261);
        assert!(msg.contains("12 of 1261 crates"), "{msg}");
        assert!(msg.contains("cargo fetch"), "{msg}");
        assert!(msg.contains("crate-00 1.0.0"), "{msg}");
        assert!(msg.contains("crate-09 1.0.0"), "{msg}");
        assert!(!msg.contains("crate-10"), "only ten are named: {msg}");
        assert!(msg.contains("… and 2 more"), "{msg}");
    }

    /// Ten or fewer are all named, with no dangling "and 0 more".
    #[test]
    fn a_short_absent_list_is_named_in_full() {
        let absent = vec![Crate {
            name: "only-one".into(),
            version: "0.1.0".into(),
        }];
        let msg = absent_source_error(&absent, 3);
        assert!(msg.contains("1 of 3 crates"), "{msg}");
        assert!(msg.contains("only-one 0.1.0"), "{msg}");
        assert!(!msg.contains("more"), "{msg}");
    }

    /// The generated header names the copyright holder, not the publisher.
    ///
    /// A holder of this binary may have neither the repository nor a release
    /// archive, so the header is where they learn whose work it is and under
    /// what terms. That is the copyright holder — Shook Law PLLC — because the
    /// notice a recipient relies on for permission has to name whoever can give
    /// it. `cli/tests/license_of_record.rs` holds the same claim across the
    /// terms files.
    #[test]
    fn rendered_header_names_the_owner_and_points_at_the_licence() {
        let notices = collect(&[], &[]);
        let out = render(&notices);
        assert!(out.contains("Shook Law PLLC"), "{out}");
        assert!(
            !out.contains(&["Neon", "Law", "Foundation"].join(" ")),
            "{out}"
        );
        assert!(out.contains("LICENSE"));
        assert!(out.contains("BUSL-1.1"));
    }
}
