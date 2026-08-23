//! Pin the licence of record: one holder, one outbound grant that holder
//! cannot withdraw, and two files that divide the work between them.
//!
//! Root `LICENSE` governs everything the Firm can license — the Rust
//! workspace, the `navigator` CLI, the build and deployment tooling, and the
//! drafted legal prose under `templates/`. One grant covers the tree, so a
//! reader never has to work out which instrument applies to the file in front of
//! them.
//!
//! **`LICENSE` is the Free Software Foundation's text and nothing else.** Not a
//! stylistic choice: a licence file is read by machines as well as people, and
//! every one of them — GitHub's own detection, `cargo deny`, an SBOM
//! generator, a corporate review team's scanner — decides which licence it is
//! looking at by comparing the file against the canonical text. Prepend a
//! paragraph and the comparison drops below the threshold, the repository page
//! stops naming the licence, and a reader who wanted one glance to see AGPL
//! instead gets an unlabelled file to read. So our own words live in `NOTICE`
//! beside it, which is where everyone else puts them.
//!
//! **`NOTICE` is where this work meets that text.** The copyright line, the
//! Foundation's right to publish, the SPDX tag, § 13 in our own voice, the
//! government forms nobody here can license, the marks the Firm reserves, and
//! the terms a contribution arrives under. `LICENSE` is the instrument; `NOTICE`
//! says how it applies here and narrows nothing.
//!
//! The Affero clause is the point rather than a detail. Section 13 obliges
//! anyone who modifies this software and lets users interact with it remotely to
//! offer those users the corresponding source, and a legal-services portal run
//! for other people is exactly that deployment shape. A change that kept the
//! SPDX tag but lost § 13 would keep the label and drop the obligation, so the
//! section is asserted by name below.
//!
//! An open-source licence is a promise to everyone who has cloned the
//! repository, and it cannot be quietly taken back: every copy keeps the rights
//! it was given, whatever a later commit says. The guard against an accidental
//! retraction is therefore structural rather than a list of forbidden clauses —
//! `LICENSE` must begin at the licence text's first line and end at its last, so
//! there is nowhere in the file for a contradicting clause to sit.
//!
//! What a licence already granted cannot promise is a *further* copy. A holder
//! is free to stop publishing, and nothing in `LICENSE` stops it — which is how
//! a project gets relicensed out from under the people building on it. Here the
//! Foundation holds a perpetual, irrevocable right to go on publishing under
//! `AGPL-3.0-only`, and that right is the reason this repository's grant outlives
//! whoever owns the Firm. It is a sentence in a file, so it is asserted below;
//! losing it would leave the grant looking identical and revocable.
//!
//! The trademark reservation is guarded just as hard, and for a reason the
//! copyright grant does not cover. Copyleft invites forks too, and a fork
//! wearing the operating firm's name would misdirect the one person least able
//! to check who is accountable for their legal work. The marks are the only
//! thing this repository withholds, so a notice that goes missing or names the
//! wrong registrant is the failure that matters most.
//!
//! Structure only, never prose. The wording is expected to keep moving; only a
//! change to the *structure* — the owner changing, a manifest drifting off the
//! tag, a terms file disappearing, the reservation going missing — lands here.

use std::fs;
use std::path::{Path, PathBuf};

/// The SPDX expression every manifest in the workspace carries.
///
/// `-only` and never `-or-later`: the terms this repository publishes under are
/// the terms in its own licence file, and no future FSF revision moves them.
const LICENSE: &str = "AGPL-3.0-only";

/// The grant itself: the Free Software Foundation's text, unaltered.
const LICENSE_FILE: &str = "LICENSE";

/// The Foundation's own statements about that grant, in the file every other
/// project keeps them in.
const NOTICE_FILE: &str = "NOTICE";

/// The copyright holder: the organization that owns this work and makes the
/// outbound grant.
///
/// A rename edits this constant and root `NOTICE` together, and nothing else in
/// this file. It is the legal person rather than the trade name on purpose: a
/// copyright notice has to name someone who can hold a copyright, and "Neon Law"
/// alone is a brand.
const OWNER: &str = "Shook Law PLLC";

/// The trademark registrant, which is currently the *same* organization as the
/// copyright holder above — and is kept a separate constant anyway.
///
/// **Asserting `REGISTRANT` no longer proves anything about `OWNER`, or the
/// reverse.** The two strings are equal, so a test that reaches for either one
/// passes on both, and the distinctness the trademark guard used to get for free
/// is gone. What still makes the constants worth keeping apart is that they name
/// two different facts about the same organization: the mark is registered, the
/// copyright is held, and either could move without the other. The load the pair
/// used to carry now sits on `PUBLISHER`, which really is somebody else.
const REGISTRANT: &str = "Shook Law PLLC";

/// The publisher: the organization holding the right to keep publishing this
/// work under the grant, and *not* the copyright holder.
///
/// This is the one genuinely two-organization fact left in the file, and the
/// only thing standing between the public grant and a change of control at the
/// Firm. A licence already granted cannot be revoked, but no holder is obliged
/// to offer another copy — so "we publish it under the AGPL" is a statement
/// about today unless somebody else is entitled to go on doing it. Somebody
/// else is, and it is asserted rather than trusted to review, because the
/// repository would look exactly the same without it.
const PUBLISHER: &str = "Neon Law Foundation";

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Whitespace-flattened, lowercased prose. Markdown wraps at the line width, so
/// a raw `contains` on a phrase reads a refilled paragraph as a deleted clause.
fn flat_lower(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The prose surrounding a match — 200 characters either side, snapped out to
/// the nearest character boundary so a multi-byte dash in the copy cannot panic
/// the slice.
fn window(body: &str, at: usize, len: usize) -> &str {
    let mut start = at.saturating_sub(200);
    while start > 0 && !body.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (at + len + 200).min(body.len());
    while end < body.len() && !body.is_char_boundary(end) {
        end += 1;
    }
    &body[start..end]
}

/// Directories that are not this workspace's own surface.
///
/// `worktrees` covers `.worktrees`, `.claude/worktrees`, and `.codex/worktrees`
/// alike. Each holds a *complete other checkout*, so walking in reads another
/// branch's files as if they were this one's. CI clones fresh and never has
/// them, which is exactly why such a failure would only ever reproduce on the
/// machine of whoever is working in a worktree.
fn is_skipped_dir(name: &str) -> bool {
    matches!(name, "target" | ".git" | "node_modules" | "vendor")
        || name.trim_start_matches('.') == "worktrees"
}

/// Every `Cargo.toml` in the workspace, including manifests the workspace
/// excludes, found by walking rather than by a hand-listed set.
fn cargo_manifests() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !is_skipped_dir(name.as_ref()) {
                    walk(&path, out);
                }
            } else if name == "Cargo.toml" {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root(), &mut out);
    out.sort();
    out
}

/// Every crate either inherits the workspace license or declares the tag.
#[test]
fn every_crate_declares_or_inherits_the_license_of_record() {
    let manifests = cargo_manifests();
    assert!(
        manifests.len() > 20,
        "expected the whole workspace, found only {} manifests — the walk is \
         probably rooted wrong",
        manifests.len()
    );

    let mut offenders = Vec::new();
    for path in &manifests {
        let body =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let doc: toml::Value =
            toml::from_str(&body).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        let package = doc
            .get("workspace")
            .and_then(|w| w.get("package"))
            .or_else(|| doc.get("package"));
        let Some(package) = package else {
            continue;
        };
        let Some(license) = package.get("license") else {
            offenders.push(format!("{}: no `license` field", path.display()));
            continue;
        };
        let inherits = license
            .get("workspace")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        if inherits {
            continue;
        }
        match license.as_str() {
            Some(LICENSE) => {}
            other => offenders.push(format!(
                "{}: license is {other:?}, expected {LICENSE:?} or \
                 `license.workspace = true`",
                path.display()
            )),
        }
    }

    assert!(
        offenders.is_empty(),
        "these manifests do not carry the workspace license of record:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn workspace_root_pins_the_license_of_record() {
    let doc: toml::Value = toml::from_str(&read("Cargo.toml")).expect("parse workspace Cargo.toml");
    let license = doc["workspace"]["package"]["license"]
        .as_str()
        .expect("workspace license is a string");
    assert_eq!(
        license, LICENSE,
        "the workspace license of record must stay `{LICENSE}`; every member \
         crate inherits this value"
    );
}

/// The VS Code extension ships outside the Cargo workspace (npm registry), so
/// its manifest declares the tag by hand and drifts on its own. It is the only
/// manifest in the tree that can.
#[test]
fn editor_extension_manifest_declares_the_license_of_record() {
    let vscode: serde_json::Value =
        serde_json::from_str(&read("lsp/vscode-ext/package.json")).expect("parse package.json");
    assert_eq!(
        vscode["license"].as_str(),
        Some(LICENSE),
        "the VS Code extension manifest must declare `{LICENSE}`"
    );
}

/// `LICENSE` is the licence text and only the licence text.
///
/// This is the guard the rest of the file leans on, and it does two jobs at
/// once.
///
/// **It keeps the licence machine-readable.** Every tool that identifies a
/// licence — GitHub's repository page, an SBOM generator, a corporate scanner —
/// compares this file against the canonical text and needs a near-exact match.
/// A paragraph of the Foundation's own prose in front of the grant is enough to
/// drop below that bar, at which point the repository stops telling a reader
/// which licence it publishes under. So the file starts at the text's first line
/// and stops at its last, and everything the Foundation has to say about the
/// grant is in `NOTICE`.
///
/// **It makes a retraction structurally impossible.** A file bounded at both
/// ends by the FSF's own lines, carrying every section between them, has nowhere
/// to hide a clause that narrows what the grant gives away — which is a stronger
/// promise than any list of forbidden wording, and it needs no maintenance.
///
/// The sections are checked by name rather than by length, because a truncated
/// paste that stopped after the definitions would still look like a licence
/// file. Each named section is one a reader relies on: § 4 obliges a conveyor to
/// hand over this License, § 6 governs conveying a built binary, § 11 is the
/// patent grant, and § 13 is the network clause that makes this the Affero
/// licence rather than the ordinary GPL.
#[test]
fn the_licence_file_is_the_grant_text_unaltered() {
    assert!(
        repo_root().join(LICENSE_FILE).exists(),
        "{LICENSE_FILE} is the licence of record and must exist"
    );
    let license = read(LICENSE_FILE);

    let first = license
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    assert_eq!(
        first, "GNU AFFERO GENERAL PUBLIC LICENSE",
        "{LICENSE_FILE} must open on the licence text's own first line. Anything \
         ahead of it — a copyright header, a scope note, a pointer to another \
         file — is what stops licence detection from naming the grant, and the \
         Foundation's own words belong in {NOTICE_FILE}."
    );

    let last = license
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    assert_eq!(
        last, "<https://www.gnu.org/licenses/>.",
        "{LICENSE_FILE} must end on the licence text's own last line, so there is \
         nowhere in the file for an added clause to sit"
    );

    for required in [
        "Version 3, 19 November 2007",
        "TERMS AND CONDITIONS",
        "0. Definitions.",
        "4. Conveying Verbatim Copies.",
        "5. Conveying Modified Source Versions.",
        "6. Conveying Non-Source Forms.",
        "11. Patents.",
        "13. Remote Network Interaction; Use with the GNU General Public License.",
        "15. Disclaimer of Warranty.",
        "END OF TERMS AND CONDITIONS",
    ] {
        assert!(
            license.contains(required),
            "{LICENSE_FILE} must carry the verbatim AGPL-3.0 text; `{required}` \
             is missing, so the grant it publishes is incomplete"
        );
    }
}

/// `NOTICE` is what ties this work to the text beside it.
///
/// The FSF's text names no author and no program, so on its own it says which
/// licence the repository publishes under and nothing about who publishes or
/// what. The copyright line, the SPDX tag, and the sentence putting *this*
/// program under the grant are what close that gap, and they live here.
#[test]
fn the_notice_puts_this_work_under_the_grant() {
    let notice = read(NOTICE_FILE);
    assert!(
        repo_root().join(NOTICE_FILE).exists(),
        "{NOTICE_FILE} carries the copyright line and our own statements about \
         the grant, and must exist"
    );
    assert!(
        notice.contains(&format!("Copyright (C) 2026 {OWNER}")),
        "{NOTICE_FILE} must carry the copyright line \
         `Copyright (C) 2026 {OWNER}` — {LICENSE_FILE} is the FSF's text and \
         names no copyright holder"
    );
    assert!(
        notice.contains(&format!("SPDX-License-Identifier: {LICENSE}")),
        "{NOTICE_FILE} must carry `SPDX-License-Identifier: {LICENSE}`"
    );

    let flat = flat_lower(&notice);
    for required in ["free software", "redistribute", "narrows the grant"] {
        assert!(
            flat.contains(required),
            "{NOTICE_FILE} must state `{required}` — it is the file that says \
             this program is published under {LICENSE} and that nothing beside \
             the grant takes anything back from it"
        );
    }
}

/// The network clause is stated where a deployer will read it.
///
/// § 13 is the whole reason this workspace is on the Affero licence rather than
/// a permissive one, and it is the obligation a reader is least likely to expect
/// from the SPDX tag alone. Someone who runs a modified Navigator as a legal
/// portal owes their users the corresponding source, and `NOTICE` has to say so
/// in the Foundation's own voice — not only inside § 13's own legalese, which a
/// deployer skims past on the way to deciding they may fork.
#[test]
fn the_notice_states_the_network_obligation_in_its_own_voice() {
    let flat = flat_lower(&read(NOTICE_FILE));

    assert!(
        flat.contains("section 13"),
        "{NOTICE_FILE} must name section 13 in its own voice — the network \
         obligation is the reason this grant is Affero, and a deployer reads a \
         short notice rather than § 13's own text"
    );
    assert!(
        flat.contains("corresponding source"),
        "{NOTICE_FILE} must say that a modified network deployment owes users \
         the corresponding source"
    );
    assert!(
        flat.contains("remotely"),
        "{NOTICE_FILE} must say the obligation attaches to letting users \
         interact with the software remotely, which is the deployment shape a \
         legal-services portal actually has"
    );
}

/// The marks are reserved, and `NOTICE` says so in the same breath as the grant.
///
/// This is the reservation the copyright grant does not make, and it is the
/// clause most likely to be lost in a rewrite, because every other sentence in
/// the file is about giving things away. A reader deciding whether they may ship
/// a fork called "Neon Law" reads the terms files, so the answer has to be in
/// one of them — and `LICENSE` is the FSF's text, which leaves this one.
#[test]
fn the_notice_reserves_the_marks_alongside_the_grant() {
    let flat = read(NOTICE_FILE)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        flat.contains("rights in copyright, not in trademarks"),
        "{NOTICE_FILE} must state that the grant covers copyright and not \
         trademarks — copyleft invites forks too, and the marks are the only \
         thing this repository withholds from one"
    );
    assert!(
        flat.contains("6,325,650"),
        "{NOTICE_FILE} must cite the NEON LAW registration it reserves"
    );
    assert!(
        flat.contains("views::brand_bundle"),
        "{NOTICE_FILE} must point a fork at the brand manifest — telling someone \
         they may not use the marks without showing them the rename seam leaves \
         patching sources as the obvious move"
    );
}

/// Exactly one licence file sits at the repository root.
///
/// The count is the assertion. One grant over the whole tree means one
/// instrument, and the way that decays is a helpful-looking sibling —
/// `LICENSE.txt` beside `LICENSE`, or a `COPYING` a tool dropped in — which
/// leaves a reader working out which of two files binds them.
///
/// `NOTICE` is not one of them and is deliberately named so: it holds no grant,
/// and no licence scanner reads it as one.
#[test]
fn the_repository_root_carries_exactly_one_licence_file() {
    let mut found: Vec<String> = fs::read_dir(repo_root())
        .expect("read repository root")
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| {
            let lower = name.to_lowercase();
            ["license", "licence", "copying"]
                .iter()
                .any(|stem| lower.starts_with(stem))
        })
        .collect();
    found.sort();

    assert_eq!(
        found,
        vec![LICENSE_FILE.to_string()],
        "the root must carry exactly one licence file, `{LICENSE_FILE}`; found \
         {found:?}. One grant covers the whole tree, so a second file can only \
         contradict the first."
    );
}

/// Contributions are closed, a contribution assigns to the Firm, and the grant
/// out is unchanged — `CONTRIBUTING.md` states all three without letting any of
/// them read as another.
///
/// These are independent facts and the file has to keep them apart. Closed is a
/// *capacity* decision about pull requests, revocable at will. Assignment is the
/// ownership position, stated in advance so a fork's authors know the terms
/// without having to ask. And the grant out is `AGPL-3.0-only` either way, which
/// is the sentence that stops the other two being misread: a reader who
/// concludes that a closed door or a signed assignment means the grant is closed
/// too has reached the one thing this repository can never say, because every
/// copy already taken keeps its rights.
///
/// Assignment is also *why* the outbound grant is durable rather than in tension
/// with it. One holder can grant the whole work; a grant assembled from many
/// holders is one nobody can reliably renew, and a contributor who could not be
/// found to re-sign would be a hole in the public grant, not in a private one.
///
/// So the closed notice must arrive with a way to reach a human. A door with no
/// address behind it is what makes an open-source project look abandoned rather
/// than deliberate, and a security report has to land somewhere.
#[test]
fn contributions_are_closed_but_the_licence_terms_are_stated_anyway() {
    /// Where someone turned away by the notice is told to write instead.
    const CONTACT: &str = "contact@neonlaw.org";

    let contributing = read("CONTRIBUTING.md");
    let flat = flat_lower(&contributing);

    assert!(
        flat.contains("closed to outside contributions"),
        "CONTRIBUTING.md must say plainly that contributions are closed; a \
         contributor should learn that before opening a pull request, not after"
    );
    assert!(
        contributing.contains(CONTACT),
        "CONTRIBUTING.md must give `{CONTACT}` as the way to reach a human — a \
         closed door with no address behind it reads as an abandoned project, \
         and a security report still has to land somewhere"
    );

    assert!(
        contributing.contains(LICENSE),
        "CONTRIBUTING.md must name `{LICENSE}` as the terms a contribution is \
         licensed under; closed to pull requests is not closed to the grant"
    );
    assert!(
        flat.contains("assigns to"),
        "CONTRIBUTING.md must state that a contribution assigns to the Firm, so \
         the terms are knowable in advance and a fork's own authors know where \
         they stand; silence on ownership reads as no assignment at all"
    );
    assert!(
        contributing.contains(OWNER),
        "CONTRIBUTING.md must name `{OWNER}` as the party a contribution assigns \
         to"
    );
    assert!(
        !flat.contains("inbound = outbound"),
        "CONTRIBUTING.md must not claim inbound = outbound: the term means the \
         inbound licence equals the outbound one and nothing further is taken, \
         and a written assignment takes more than that. The grant out is still \
         `{LICENSE}` — say that instead of a term that now reads as a promise \
         there is no agreement to sign"
    );
}

/// The Foundation's right to keep publishing is stated where a reader relies on
/// it.
///
/// This is the assertion the rest of the file cannot make for you. Every other
/// check here confirms that the grant *is* `AGPL-3.0-only` today, and every one
/// of them would stay green if the copyright holder decided tomorrow to publish
/// nothing further. A licence already granted cannot be revoked — but no holder
/// owes anyone the next copy, which is the whole mechanism behind every
/// relicensing a community has been angry about.
///
/// What closes that gap is a second organization entitled to go on publishing.
/// The Foundation holds a perpetual, irrevocable, royalty-free right to publish
/// this work under the grant; it binds the Firm's successors and survives a
/// change of the Firm's control. That promise lives in prose, in two files, and
/// nothing about the repository would look different if it were quietly dropped
/// — which is exactly the shape of claim that needs a test rather than a
/// reviewer.
///
/// `assert_ne!` on the two organizations is the load-bearing line. The point is
/// not that a particular charity is named; it is that the publisher is somebody
/// other than the holder, because a right the holder grants itself is no
/// constraint on the holder at all.
#[test]
fn the_publication_right_is_stated_and_held_by_someone_other_than_the_owner() {
    /// Every word that has to describe the right, and what each one buys.
    ///
    /// Not decoration. Drop `perpetual` and it can expire; drop `irrevocable`
    /// and the Firm can end it; drop `royalty-free` and the Foundation can be
    /// priced out of exercising it. Any one of them missing leaves a right the
    /// Firm can outlast.
    const TERMS: [&str; 3] = ["perpetual", "irrevocable", "royalty-free"];

    /// That an acquisition does not end it. Either spelling will do.
    const SURVIVAL: [&str; 2] = ["successors", "change of"];

    assert_ne!(
        OWNER, PUBLISHER,
        "the publication right has to sit with an organization other than the \
         copyright holder; a holder cannot meaningfully bind itself, and a \
         reader relying on the grant's durability is relying on a third party \
         being able to enforce it"
    );

    for rel in [NOTICE_FILE, "README.md", "docs/licensing.md"] {
        // Whitespace-flattened but not lowercased: the organization is matched
        // by name, and only the window around it is folded for the terms.
        let flat = read(rel).split_whitespace().collect::<Vec<_>>().join(" ");
        let named: Vec<usize> = flat.match_indices(PUBLISHER).map(|(i, _)| i).collect();

        assert!(
            !named.is_empty(),
            "{rel} must name `{PUBLISHER}` as the organization holding the right \
             to publish this work"
        );

        // At least one mention has to carry the whole description. Checking the
        // file rather than a window is what makes this guard hollow: each of
        // these words appears elsewhere in prose about the right, so a
        // file-wide `contains` stays green while the sentence that actually
        // grants it loses a term.
        let described = named.iter().any(|&at| {
            let claim = window(&flat, at, PUBLISHER.len()).to_lowercase();
            TERMS.iter().all(|term| claim.contains(term))
                && SURVIVAL.iter().any(|clause| claim.contains(clause))
        });

        assert!(
            described,
            "{rel} names `{PUBLISHER}` but no mention of it describes the right \
             in full. One sentence has to carry every one of {TERMS:?} and say \
             the right survives a change of control ({SURVIVAL:?}): a right that \
             expires, that the Firm can revoke, that can be priced out of use, or \
             that dies with an acquisition protects nobody — acquisition is the \
             thing it exists to protect against."
        );
    }
}

#[test]
fn readme_states_the_license_of_record() {
    let readme = read("README.md");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains(OWNER),
        "README.md must name `{OWNER}` as the copyright holder of Neon Law Navigator"
    );
    assert!(
        flat.contains(LICENSE),
        "README.md must name `{LICENSE}` as the software's licence"
    );
}

/// One grant covers the whole tree, `templates/` included.
///
/// A reader of a notation body is under the same licence as a reader of the code
/// that renders it, and `templates/README.md` has to say so where a notation
/// author will actually see it — someone editing a template is inside
/// `templates/`, not at the repository root.
///
/// The carve-out inside the tree is not a licensing choice at all: the blank
/// government PDFs under `templates/forms/` are the issuing agency's work. An
/// AGPL grant over a Nevada state form would claim a copyright nobody here
/// holds, and an over-claim in a law firm's own terms file is the kind of error
/// that is quoted back.
#[test]
fn the_single_grant_covers_the_templates_tree() {
    let flat = read(NOTICE_FILE)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        flat.contains("templates/"),
        "{NOTICE_FILE} must say that the grant reaches `templates/` — the \
         drafted prose is licensed with the software, and silence there is what \
         sends a reader looking for a second instrument"
    );
    assert!(
        flat.contains("templates/forms/"),
        "{NOTICE_FILE} must carve out the government forms under \
         `templates/forms/` — they are the issuing agency's work and nobody \
         here grants anything in them"
    );

    // The tree the notice describes has to exist, or it is describing a layout
    // the repository no longer has.
    assert!(
        repo_root().join("templates").is_dir(),
        "{NOTICE_FILE} names `templates/`; that tree must exist"
    );
    assert!(
        repo_root().join("templates/forms").is_dir(),
        "{NOTICE_FILE} carves out `templates/forms/`; that tree must exist"
    );

    // The tree states its own terms, because someone reading a notation is
    // usually inside `templates/` and not at the repository root.
    assert!(
        read("templates/README.md").contains(LICENSE),
        "templates/README.md must state `{LICENSE}` where an author of a \
         notation will actually see it"
    );
}

/// Every published image declares the licence and carries both terms files.
///
/// A container image someone pulled is a copy, and its holder has neither the
/// repository nor a release archive. AGPL § 4 conditions the permission to
/// convey on handing every recipient this License along with the work, and § 13
/// may oblige that holder to pass the source on in turn — which they cannot do
/// from terms they were never shown. Three mechanisms, because they serve
/// different readers: the OCI label is what a registry page shows before anyone
/// pulls, and the two staged files are what a running container can actually be
/// made to print.
///
/// `Containerfile.runner` is exempt. It is the CI runner image rather than a
/// published artifact of the software, and it has no distroless runtime stage
/// to stage anything into.
#[test]
fn every_published_image_declares_the_licence_and_stages_its_text() {
    let images = repo_root().join("images");
    let mut offenders = Vec::new();

    for entry in fs::read_dir(&images).expect("read images/").flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("Containerfile.") || name == "Containerfile.runner" {
            continue;
        }
        let body = fs::read_to_string(&path).expect("read Containerfile");

        for required in [
            &format!("org.opencontainers.image.licenses=\"{LICENSE}\"") as &str,
            &format!("COPY {LICENSE_FILE} /app/{LICENSE_FILE}") as &str,
            &format!("COPY {NOTICE_FILE}  /app/{NOTICE_FILE}") as &str,
        ] {
            if !body.contains(required) {
                offenders.push(format!("{name}: missing `{required}`"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "every published image must declare {LICENSE} and carry both terms \
         files; a puller holds no repository and no archive:\n  {}",
        offenders.join("\n  ")
    );
}

/// The GHCR push is unconditional, and the scope it needs is granted per job.
///
/// **Unconditional.** GHCR is where every published image lives — `ops ship`
/// renders `ghcr.io/<owner>` into every `image:` line, and the CLI's
/// `DEFAULT_REGISTRY` names it — so the push is not an optional mirror that a
/// toggle may switch off. A repository *variable* is the worst possible switch
/// for it: clearing one is a settings edit that touches no file, passes every
/// gate, and produces a release that looks fine until someone checks the
/// registry days later. The images simply would not be there.
///
/// So no condition may guard the push, and this test is what stops one being
/// reintroduced. The package visibility that once justified a gate is settled:
/// a GHCR package inherits its linked repository's visibility and
/// `neon-law-foundation/navigator` is public.
///
/// **Scoped to the publish jobs.** `packages: write` belongs on the two jobs
/// that push an image. Granting it at the top of the workflow would hand the
/// scope to every job in the file, including the ones that check out and build
/// arbitrary release code.
#[test]
fn the_image_push_is_unconditional_and_scoped_to_the_publish_jobs() {
    let workflow = read(".github/workflows/deploy.yml");

    for required in ["registry: ghcr.io", "password: ${{ secrets.GITHUB_TOKEN }}"] {
        assert!(
            workflow.contains(required),
            "deploy.yml must retain the keyless GHCR push contract `{required}`"
        );
    }

    // No toggle, under any spelling. `vars.` is the whole namespace a repository
    // variable can be read through, so excluding it from the publish jobs
    // catches a gate reintroduced under a different variable name too.
    //
    // Scoped to those jobs rather than the file: `notify` reads `vars.DEPLOY_REPO`
    // legitimately, and unset there means one Slack line reads "your deployments
    // checkout" instead of a name. Narrating a rollout is not publishing it.
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&workflow).expect("deploy.yml parses as YAML");
    for job in ["publish-service", "publish-triggers"] {
        // Re-serialised from the parse so YAML comments are not part of the
        // text searched: the prose in this file necessarily names the toggle it
        // forbids, and the job's CONFIGURATION is what must not carry it.
        let body = serde_yaml::to_string(&parsed["jobs"][job])
            .unwrap_or_else(|error| panic!("`{job}` re-serialises: {error}"));
        assert!(
            !body.contains("vars."),
            "`{job}` must read no repository variable: publishing must not depend on one, \
             whose absence is a silent, file-free way to stop a release reaching the registry"
        );
    }
    assert!(
        !workflow.contains("GHCR_PUBLISH"),
        "the GHCR push carries no toggle; `GHCR_PUBLISH` must not return in any form"
    );

    // The composed image list must be composed unconditionally. A `if` inside
    // the heredoc is how the toggle was previously spelled, and it is the one
    // that decides what `metadata-action` is handed.
    let composed = workflow
        .split("- name: compose registry list")
        .nth(1)
        .expect("deploy.yml must keep its `compose registry list` step")
        .split("- name: image metadata")
        .next()
        .expect("the composed list is followed by the metadata step");
    assert!(
        !composed.contains("if ["),
        "the registry list must be composed unconditionally; a conditional branch here is how \
         a publish is silently narrowed to nothing"
    );

    // The scope must be granted, and granted narrowly. A `permissions:` block
    // at column 2 is the workflow's; at column 4 it is a job's.
    let granted: Vec<&str> = workflow
        .lines()
        .filter(|line| line.trim() == "packages: write")
        .collect();
    // Both jobs that push an image hold the scope: `publish-service` and
    // `publish-triggers`. What must not change is that each grant is a JOB's —
    // the count is allowed to track the publish jobs, the indent is not.
    assert_eq!(
        granted.len(),
        2,
        "`packages: write` is expected on the two publish jobs, no others"
    );
    for grant in &granted {
        assert!(
            grant.starts_with("      packages: write"),
            "`packages: write` must be granted at job level (six-space indent), \
             not at the top of the workflow where every job would inherit it"
        );
    }

    // One build, every tag. `metadata-action` fans the tags across every image
    // it is given, so the push step must read the composed list rather than a
    // single name — a second push step would mean a second full compile of
    // identical layers.
    assert!(
        workflow.contains("images: ${{ steps.imgs.outputs.list }}"),
        "the publish job's metadata step must read the composed registry list, so one build \
         serves every tag it publishes"
    );
}

/// Public surfaces that name the NEON LAW registration attribute it to the Firm.
///
/// U.S. Reg. No. 6,325,650 belongs to the Firm, and the Firm licenses it to the
/// Neon Law Foundation for its charitable work. A trademark notice that names
/// the wrong owner is worse than none at all, because it is the notice a reader
/// relies on for permission — and under an outbound grant that reliance is no
/// longer hypothetical, since the licence invites forks and the mark is the one
/// thing it withholds from them. The registration number is the anchor: a
/// surface may mention the mark without it, but a surface that cites the number
/// is making an ownership claim and must make the right one.
///
/// This asserts `REGISTRANT`, which now equals `OWNER` — so unlike before, it no
/// longer also proves the copyright holder is somebody else. It cannot: they are
/// the same organization. The check that survives is the narrow one it was always
/// making, that a surface citing the number names the registrant, and the reader
/// who needs to know a copyright licence does not reach a mark is served by
/// `docs/licensing.md` rather than by a string comparison here.
#[test]
fn trademark_notices_name_the_firm_as_the_registrant() {
    /// The registration itself, used as the anchor for "this line claims
    /// ownership" rather than merely naming the brand.
    const REGISTRATION: &str = "6,325,650";

    let mut offenders = Vec::new();
    for rel in [
        NOTICE_FILE,
        "README.md",
        "docs/glossary.md",
        // Where the ownership claim actually lives. `NOTICE` names the mark but
        // not every surface does, so this is the doc that makes the numbered
        // claim the licence deliberately does not grant.
        "docs/licensing.md",
        "templates/README.md",
        // One binary serves the firm at the root and the Foundation under
        // `/foundation`, so one bundled terms file carries the citation for
        // both faces.
        "neon/content/terms.md",
    ] {
        // Prose wraps at the Markdown line width, so a claim routinely straddles
        // a line break, and "U.S. Reg. No." defeats splitting on sentence ends.
        // Collapse whitespace and read a window around each citation instead.
        let flat = read(rel).split_whitespace().collect::<Vec<_>>().join(" ");
        let citations: Vec<usize> = flat.match_indices(REGISTRATION).map(|(i, _)| i).collect();
        assert!(
            !citations.is_empty(),
            "{rel} cites the NEON LAW registration and is guarded here; if the \
             citation moved, move this list with it"
        );
        for at in citations {
            let claim = window(&flat, at, REGISTRATION.len());
            if !claim.contains(REGISTRANT) {
                offenders.push(format!(
                    "{rel}: cites {REGISTRATION} without naming `{REGISTRANT}` — …{claim}…"
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "NEON LAW is registered to {REGISTRANT}; these notices say otherwise:\n  {}",
        offenders.join("\n  ")
    );
}

/// Every Markdown document in the tree, so a claim about the grant is guarded
/// wherever someone writes it rather than only in the terms files.
fn markdown_documents() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !is_skipped_dir(name.as_ref()) {
                    walk(&path, out);
                }
            } else if name.ends_with(".md") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root(), &mut out);
    out.sort();
    out
}

/// No document reads § 13 as a duty owed to this project or to the public.
///
/// § 13 runs in exactly one direction: an operator who modified the software and
/// lets users interact with it remotely must offer *those users* — the people
/// using that operator's instance — the corresponding source. It obliges no
/// publication to the world and nothing at all upstream. Nothing comes back
/// here as a matter of licence.
///
/// Writing it the other way is the flattering mistake, because "forks come back"
/// is the story a copyleft project likes to tell about itself. It is also the
/// costly one. A firm's counsel reading a claim that deploying obliges them to
/// publish is reading an obligation the licence does not impose, and a terms
/// document published beside a law practice over-claiming what its own licence
/// requires is the kind of error that gets quoted back. The direction of the
/// duty is the substance of the clause, so it is asserted rather than trusted to
/// review — this repository stated it backwards in three places at once while
/// every other document had it right.
///
/// Three assertions, because a prose guard fails in three different ways. The
/// misreadings must be absent; the walk must actually have found the documents
/// (a guard that inspects nothing passes forever); and a document that raises
/// § 13 must name who is owed, since the recipients are what makes the
/// obligation the operator's rather than the public's.
#[test]
fn no_document_reads_section_13_as_a_duty_to_this_project_or_the_public() {
    /// How a reader names the clause.
    ///
    /// The section number is the obvious anchor and the incomplete one: a
    /// marketing page reaches for "the Affero clause" instead, which is how one
    /// misstatement outlived the first version of this guard. Both spellings of
    /// the number appear in the tree, and so does the name.
    const CITATIONS: [&str; 3] = ["§ 13", "section 13", "affero clause"];

    /// Phrasings that state the wrong obligation, each with what it gets wrong.
    ///
    /// Matched inside a window around a § 13 citation, so ordinary uses
    /// elsewhere in a document are not the guard's business.
    const MISREADINGS: [(&str, &str); 10] = [
        ("come back", "§ 13 sends nothing back to this project"),
        ("comes back", "§ 13 sends nothing back to this project"),
        ("contribute back", "§ 13 obliges no contribution upstream"),
        ("upstream", "§ 13 obliges nothing upstream"),
        (
            "publish those improvements",
            "§ 13 obliges an offer of source to that operator's own users, not publication",
        ),
        (
            "publishes those improvements",
            "§ 13 obliges an offer of source to that operator's own users, not publication",
        ),
        (
            "publish your changes",
            "§ 13 obliges an offer of source to that operator's own users, not publication",
        ),
        (
            "to the public",
            "§ 13 runs to the operator's own remote users, not to the public",
        ),
        (
            "puts theirs back",
            "§ 13 puts nothing back into the commons; it owes source to that operator's own users",
        ),
        (
            "put theirs back",
            "§ 13 puts nothing back into the commons; it owes source to that operator's own users",
        ),
    ];

    /// What names the people actually owed the source.
    ///
    /// Several spellings, because the register changes with the surface: a terms
    /// file says "those users" and a marketing page says the clients using it.
    /// Naming them is the assertion — which of the words is used is not.
    const RECIPIENTS: [&str; 6] = [
        "those users",
        "your users",
        "those clients",
        "clients using it",
        "its own users",
        "their own users",
    ];

    let mut citing = Vec::new();
    let mut offenders = Vec::new();

    for path in markdown_documents() {
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace("../", "");
        let flat = flat_lower(&fs::read_to_string(&path).unwrap_or_default());

        let mut cites = false;
        for citation in CITATIONS {
            for (at, hit) in flat.match_indices(citation) {
                cites = true;
                let claim = window(&flat, at, hit.len());
                for (misreading, why) in MISREADINGS {
                    if claim.contains(misreading) {
                        offenders.push(format!(
                            "{rel}: says \"{misreading}\" beside {citation} — {why}: \u{2026}{claim}\u{2026}"
                        ));
                    }
                }
            }
        }

        if cites {
            citing.push(rel.clone());
            if !RECIPIENTS.iter().any(|who| flat.contains(who)) {
                offenders.push(format!(
                    "{rel}: raises \u{a7} 13 without naming who is owed the source; say it \
                     runs to those users, or the reader reads it as a duty to the public"
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "\u{a7} 13 obliges an operator to offer corresponding source to its own remote \
         users \u{2014} nothing upstream and nothing to the public:\n  {}",
        offenders.join("\n  ")
    );

    // The walk has to have read the document that explains the clause. A prose
    // guard whose corpus quietly emptied \u{2014} a renamed file, a skipped directory
    // \u{2014} reports success while inspecting nothing.
    assert!(
        citing.contains(&"docs/licensing.md".to_string()),
        "docs/licensing.md is where \u{a7} 13 is explained, so it must be among the \
         documents this guard read; found: {citing:?}"
    );
}

/// Work by the firm's own people assigns to the firm that engaged them.
///
/// `CONTRIBUTING.md` states the assignment and the grant out, and a reader has
/// to be able to tell them apart. Every author in this repository signed an
/// employment or contractor agreement assigning their work before their first
/// commit, and those agreements are with Shook Law PLLC, the firm that engages
/// them and carries the bar licence. What reaches a reader is unchanged by any
/// of that: the work is published under `AGPL-3.0-only` on the same terms as
/// everything else here.
///
/// Naming the wrong assignee here is not a cosmetic error. An assignment of
/// copyright is only effective by a written instrument signed by the owner
/// (17 U.S.C. § 204(a)), so the entity named is the entity the instrument names
/// or the sentence describes a conveyance that never happened. This file is also
/// the first thing a fork's counsel reads to work out who could license them
/// anything, which makes it the worst place in the tree to name an assignee the
/// signed agreements do not.
///
/// Three assertions, because the sentence fails in three ways: it can vanish,
/// it can lose the firm, or it can name the wrong organization outright. The
/// last is the mistake that was actually made, and the first is what would let a
/// silent deletion pass as a fix.
#[test]
fn the_internal_assignment_names_the_firm_that_engaged_the_author() {
    /// Where the sentence describes the conveyance.
    const ANCHOR: &str = "assigns to";

    // Prose wraps at the Markdown line width, so the entity routinely lands on
    // the line after the verb. Collapse whitespace and read a window instead of
    // matching a phrase against a wrapped line.
    let flat = read("CONTRIBUTING.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let claims: Vec<usize> = flat.match_indices(ANCHOR).map(|(i, _)| i).collect();

    assert!(
        !claims.is_empty(),
        "CONTRIBUTING.md must keep saying that work by the practice's own \
         personnel and contractors assigns to the firm that engaged them; a \
         fork's counsel reads this file to work out who can license them \
         anything, and silence reads as no assignment at all"
    );

    let mut offenders = Vec::new();
    for at in claims {
        let claim = window(&flat, at, ANCHOR.len());
        if !claim.contains(REGISTRANT) {
            offenders.push(format!(
                "names no assignee, or the wrong one — `{REGISTRANT}` is the \
                 party the agreements name: …{claim}…"
            ));
        }
        if claim.contains(&format!("{ANCHOR} the Neon Law Foundation"))
            || claim.contains(&format!("{ANCHOR} the Foundation"))
        {
            offenders.push(format!(
                "assigns to the Foundation, which holds no such agreement — \
                 the employment and contractor agreements are `{REGISTRANT}`'s: \
                 …{claim}…"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "the practice engages its personnel and contractors through \
         `{REGISTRANT}`, so that is the assignee:\n  {}",
        offenders.join("\n  ")
    );
}
