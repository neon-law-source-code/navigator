//! Pin the licence of record: one holder, one source-available grant with its
//! four parameters filled in, and two files that divide the work between them.
//!
//! Root `LICENSE` governs everything the Firm can license — the Rust
//! workspace, the `navigator` CLI, the build and deployment tooling, and the
//! drafted legal prose under `templates/`. One grant covers the tree, so a
//! reader never has to work out which instrument applies to the file in front of
//! them.
//!
//! **`LICENSE` is the licence text and nothing else.** Not a stylistic choice: a
//! licence file is read by machines as well as people, and every one of them —
//! GitHub's own detection, `cargo deny`, an SBOM generator, a corporate review
//! team's scanner — decides which licence it is looking at by comparing the file
//! against the canonical text. Prepend a paragraph and the comparison drops
//! below the threshold, the repository page stops naming the licence, and a
//! reader who wanted one glance to see BUSL instead gets an unlabelled file to
//! read. So our own words live in `NOTICE` beside it, which is where everyone
//! else puts them.
//!
//! BUSL is a *template*, which makes this a narrower rule than it looks. Its
//! four parameters — Licensor, Licensed Work, Additional Use Grant, Change Date,
//! Change License — are filled in by the licensor and are part of the
//! instrument, so the Firm's own name legitimately appears in `LICENSE` where
//! under the FSF's text it never could. Everything *outside* that block is
//! copied unaltered, which is what BUSL's fourth Covenant of Licensor requires.
//!
//! **`NOTICE` is where this work meets that text.** The copyright line, the SPDX
//! tag, what the four parameters mean in this project's own voice, the
//! production boundary, the government forms nobody here can license, the marks
//! the Firm reserves, and the terms a contribution arrives under. `LICENSE` is
//! the instrument; `NOTICE` says how it applies here and neither widens nor
//! narrows it.
//!
//! The parameters are the point rather than a detail, and two of them carry the
//! whole commercial position. `Additional Use Grant: None` is what makes every
//! production use need a commercial licence — BUSL's base grant already covers
//! non-production use, so the Additional Use Grant is the slot for permitting
//! limited production use, and `None` is the absence of that extra permission
//! rather than a restriction added to the licence. `Change Date` is what stops
//! the arrangement being permanent: each published version converts to
//! `AGPL-3.0-only` four years on, per version, whatever happens to later ones.
//! A change that kept the SPDX tag but lost either parameter would keep the
//! label and move the deal, so both are asserted by value below.
//!
//! This work was published under `AGPL-3.0-only` before this licence took
//! effect, and every copy distributed then is still an `AGPL-3.0-only` copy,
//! permanently. A licence already granted cannot be withdrawn, so the relicence
//! governs versions published from here on and reaches nothing already given.
//! The Firm is the copyright holder and the sole Licensor. A second
//! organization entitled to publish would make that untrue while every other
//! check here stayed green, so its absence is asserted below rather than
//! assumed.
//!
//! The trademark reservation is guarded just as hard, and for a reason the
//! copyright grant does not cover. The licence permits forks, and a fork wearing
//! the operating firm's name would misdirect the one person least able to check
//! who is accountable for their legal work. BUSL declines to grant trademark
//! rights in its own terms; `NOTICE` says whose marks those are and where the
//! rename seam is, so a notice that goes missing or names the wrong registrant
//! is the failure that matters most.
//!
//! Structure only, never prose. The wording is expected to keep moving; only a
//! change to the *structure* — the owner changing, a manifest drifting off the
//! tag, a terms file disappearing, the reservation going missing — lands here.

use std::fs;
use std::path::{Path, PathBuf};

/// The SPDX expression every manifest in the workspace carries.
///
/// Source-available rather than open source. BUSL grants copying, modification,
/// and non-production use; production use needs a commercial licence, and each
/// version converts to [`CHANGE_LICENSE`] on its own Change Date.
const LICENSE: &str = "BUSL-1.1";

/// What each version converts to, four years after it is published.
///
/// Asserted separately from [`LICENSE`] because it is a different promise: the
/// outbound grant today, and the grant every published version becomes whatever
/// the Firm later decides. BUSL's Covenants of Licensor oblige this to be
/// GPL-2.0-or-later or something compatible with a later version of it, which
/// AGPL-3.0 satisfies through GPL-3.0 § 13.
const CHANGE_LICENSE: &str = "AGPL-3.0-only";

/// The grant itself: the Business Source License 1.1, parameters filled in and
/// terms otherwise unaltered.
const LICENSE_FILE: &str = "LICENSE";

/// The Firm's own statements about that grant, in the file every other project
/// keeps them in.
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
/// copyright is held, and either could move without the other.
const REGISTRANT: &str = "Shook Law PLLC";

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

/// [`flat_lower`], with Markdown emphasis markers removed as well.
///
/// A phrase match against Markdown reads `**Neon Law Foundation**` and
/// `Neon Law Foundation` as different strings, so a claim written in bold slips
/// past a matcher built for plain prose — and the claims most worth catching are
/// the emphasized ones, because emphasis is what a writer reaches for on the
/// sentence they think matters.
fn unemphasized(body: &str) -> String {
    flat_lower(body).replace(['*', '_'], "")
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

/// `LICENSE` is the licence text, its parameters, and nothing else.
///
/// This is the guard the rest of the file leans on, and it does two jobs at
/// once.
///
/// **It keeps the licence machine-readable.** Every tool that identifies a
/// licence — GitHub's repository page, an SBOM generator, a corporate scanner —
/// compares this file against the canonical text and needs a near-exact match.
/// A paragraph of the Firm's own prose in front of the grant is enough to drop
/// below that bar, at which point the repository stops telling a reader which
/// licence it publishes under. So the file opens on the licence's own title,
/// closes on its last covenant, and everything the Firm has to say about the
/// grant is in `NOTICE`.
///
/// **It pins the four parameters by value.** This is the half that has no
/// analogue under a licence like the AGPL, whose text is invariant. BUSL is a
/// template: the same unaltered terms produce a completely different deal
/// depending on what the parameters say, so a file that is textually perfect and
/// carries `Additional Use Grant: anyone, for anything` would pass a scanner and
/// give the product away. The values are therefore asserted, not just the
/// headings.
///
/// The terms are checked by phrase rather than by length, because a truncated
/// paste that stopped after the first grant would still look like a licence
/// file. Each phrase is one a reader relies on: the base grant, the conversion
/// on the Change Date, the requirement to buy a licence for non-complying use,
/// the trademark carve-out, and the four Covenants of Licensor that bound what a
/// licensor may put in the parameters at all.
#[test]
fn the_licence_file_is_the_grant_text_with_our_parameters() {
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
        first, "Business Source License 1.1",
        "{LICENSE_FILE} must open on the licence's own title. Anything ahead of \
         it — a copyright header, a scope note, a pointer to another file — is \
         what stops licence detection from naming the grant, and the Firm's own \
         words belong in {NOTICE_FILE}."
    );

    let last = license
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    assert_eq!(
        last, "4. Not to modify this License in any other way.",
        "{LICENSE_FILE} must end on the licence text's own last covenant, so \
         there is nowhere in the file for an added clause to sit"
    );

    // The invariant body. BUSL's fourth covenant is "not to modify this License
    // in any other way", so every one of these is copied rather than drafted.
    for required in [
        "Terms",
        "the right to copy, modify, create derivative",
        "make non-production use of the Licensed Work",
        "Effective on the Change Date",
        "you must purchase a\ncommercial license from the Licensor",
        "This License does not grant you any right in any trademark or logo of",
        "Covenants of Licensor",
        "To specify as the Change License the GPL Version 2.0 or any later version",
        "To specify a Change Date.",
    ] {
        assert!(
            license.contains(required),
            "{LICENSE_FILE} must carry the verbatim BUSL-1.1 text; `{required}` \
             is missing, so the grant it publishes is incomplete"
        );
    }

    // The parameters block, by value. Each line is the deal rather than
    // boilerplate, and the reason each one matters is in `NOTICE`.
    for (required, why) in [
        (
            format!("Licensor:             {OWNER}"),
            "the Licensor is who a production user buys a licence from, and who \
             may set every other parameter",
        ),
        (
            "Licensed Work:        Neon Law Navigator".to_string(),
            "the Licensed Work names what is licensed; an unnamed work leaves \
             the scope of the grant to argument",
        ),
        (
            "Additional Use Grant: None".to_string(),
            "`None` is what makes production use need a commercial licence. Any \
             prose here grants production rights away, and BUSL's second \
             covenant allows only a no-restriction grant or this literal word",
        ),
        (
            "Change Date:          Four years from the date the Licensed Work is published"
                .to_string(),
            "the Change Date is when each version stops being restricted; \
             removing it leaves the restriction permanent",
        ),
        (
            format!("({CHANGE_LICENSE})"),
            "the Change License is what each version converts into, and BUSL's \
             first covenant bounds what it may be",
        ),
    ] {
        assert!(
            license.contains(&required),
            "{LICENSE_FILE}: {why} (missing `{required}`)"
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
    for required in [
        // Said plainly, because the SPDX tag alone reads as open source to
        // anyone who has only ever seen OSI licences in that field.
        "source-available",
        "not open source",
        "redistribute",
        "nothing here widens the grant",
    ] {
        assert!(
            flat.contains(required),
            "{NOTICE_FILE} must state `{required}` — it is the file that says \
             this program is published under {LICENSE} and that nothing beside \
             the grant adds to or takes from it"
        );
    }
}

/// The production boundary is stated where a deployer will read it.
///
/// This is the obligation a reader is least likely to expect from the SPDX tag
/// alone, and under BUSL it is the whole licence rather than one clause of it:
/// someone who runs Navigator to deliver legal services to other people needs a
/// commercial licence first. BUSL never defines "production use", so a notice
/// that repeats the term without drawing the line leaves every reader to guess —
/// and the guess a developer makes standing in front of a source tree is the
/// permissive one.
///
/// The § 13 history is asserted alongside it for the opposite reason. This work
/// was Affero-licensed, that obligation is what a returning reader remembers,
/// and the honest answer is that it is gone now and comes back at the Change
/// Date. A notice silent on it reads as an oversight.
#[test]
fn the_notice_draws_the_production_boundary_in_its_own_voice() {
    let flat = flat_lower(&read(NOTICE_FILE));

    for (required, why) in [
        (
            "production use",
            "the notice must name the thing that needs buying, in the licence's \
             own vocabulary",
        ),
        (
            "commercial licence",
            "it must say what a production user has to obtain, rather than only \
             that production use is not granted",
        ),
        (
            "non-production",
            "it must say what *is* granted; a boundary stated from one side \
             reads as a blanket prohibition",
        ),
        (
            "evaluating it",
            "the notice must give worked examples of the free side of the line. \
             BUSL defines no boundary, so a reader who cannot place their own \
             case has nothing to rely on but the phrase",
        ),
        (
            "§ 13",
            "the notice must account for the Affero obligation this work used to \
             carry — it is what a returning reader looks for first",
        ),
        (
            "change date",
            "and say when it returns, because the conversion is the answer to \
             \"is this permanent\"",
        ),
    ] {
        assert!(
            flat.contains(required),
            "{NOTICE_FILE}: {why} (missing `{required}`)"
        );
    }
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

/// No document claims a second party may publish this work.
///
/// This inverts an assertion that used to sit here, and the inversion is the
/// point. A second organization once held a perpetual, irrevocable,
/// royalty-free right to publish this work under `AGPL-3.0-only` that bound the
/// Firm's successors, and while it was in force a repository naming `BUSL-1.1`
/// as its sole grant would have been describing terms that were not the only
/// ones in effect. Nothing holds such a right now, and the tree does not
/// discuss it: the Firm is the copyright holder and the sole Licensor, which is
/// a complete statement without a history lesson attached.
///
/// What survives is the guard, because the failure mode did not go away. A
/// second organization entitled to publish is exactly the claim that makes a
/// stated licence untrue while every other check stays green — and it is the
/// kind of sentence that returns by being copied out of an older file. So the
/// assertion is that no document anywhere describes one, checked across every
/// Markdown file rather than a list this test keeps in step.
#[test]
fn no_document_claims_a_second_party_may_publish_this_work() {
    /// The shapes such a claim arrives in.
    ///
    /// Matched against prose with whitespace collapsed, case folded, and
    /// emphasis stripped — the wording most worth catching is the emphasized
    /// kind, because emphasis is what a writer reaches for on the sentence they
    /// think matters.
    const CLAIMS: [&str; 4] = [
        "right to publish",
        "right to keep publishing",
        "entitled to go on publishing",
        "publication right",
    ];

    let mut offenders = Vec::new();
    for path in markdown_documents() {
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let body = unemphasized(&read(&rel));
        for claim in CLAIMS {
            for (at, hit) in body.match_indices(claim) {
                let window = window(&body, at, hit.len());
                // The Firm holding its own copyright is not a second party, and
                // neither is a sentence saying nobody else holds one.
                if window.contains("no second") || window.contains("sole licensor") {
                    continue;
                }
                offenders.push(format!("{rel}: says `{claim}` — …{window}…"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "{OWNER} is the sole Licensor; a second party entitled to publish would \
         make `{LICENSE}` untrue as the licence of record:\n  {}",
        offenders.join("\n  ")
    );
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
/// repository nor a release archive. BUSL obliges you to display this License
/// conspicuously on every copy of the Licensed Work, and its parameters are what
/// tell that holder whether their own use needs a commercial licence — which
/// they cannot work out from terms they were never shown. Three mechanisms,
/// because they serve different readers: the OCI label is what a registry page
/// shows before anyone pulls, and the two staged files are what a running
/// container can actually be made to print.
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
/// `neon-law-source-code/navigator` is public.
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
/// Only the copyright holder may sell a production licence, and no price is
/// published.
///
/// Under the AGPL this section described an *optional extra*: running, forking,
/// and redistributing were already free, and a commercial licence bought relief
/// from copyleft for someone who wanted it. Under BUSL the same section describes
/// something a production user has no way around — the licence is the permission,
/// not a convenience — so the sentence that used to matter most here ("this
/// restricts nothing in the public grant") is now false and must not reappear.
///
/// What survives unchanged is who may grant it. A production exception is carved
/// out of the copyright, so the holder of the copyright is the only party with
/// anything to carve; there is no second organization with a licence to
/// sublicense from, and after the relicence there is no second organization at
/// all.
///
/// The no-price rule is asserted alongside it. The consumer flat fees are
/// published in full and deliberately; a deployment's scope is not knowable in
/// advance, so a figure here would be a floor dressed as a fee.
#[test]
fn only_the_copyright_holder_may_sell_a_production_licence() {
    /// The prose that has to be present, and what each part of it prevents.
    const REQUIRED: [(&str, &str); 4] = [
        (
            "only the copyright holder",
            "the section must say the licence is the holder's alone to grant",
        ),
        (
            "production use",
            "it must name what triggers the need to buy, in the licence's own \
             vocabulary rather than a paraphrase",
        ),
        (
            "non-production use needs no licence",
            "it must say what does *not* need buying. A section about paying, \
             silent on the free side, reads as though reading the source were \
             chargeable",
        ),
        (
            "no price is published",
            "a deployment is quoted per engagement, and the section must say so \
             rather than leaving room for a figure",
        ),
    ];

    /// Claims that were true under the AGPL and are false under BUSL.
    ///
    /// This is the guard against a half-updated document: the old section's
    /// reassurance survives a find-and-replace on the licence name, and it is
    /// exactly the sentence a production user would rely on.
    const STALE: [(&str, &str); 2] = [
        (
            "restriction on the public grant",
            "under BUSL production use *is* restricted; this sentence told a \
             reader the opposite and was true only while the grant was AGPL",
        ),
        (
            "legal aid organizations at cost",
            "that was a sublicensing power held by a second organization, and \
             no second organization holds anything to sublicense from",
        ),
    ];

    let doc = "docs/licensing.md";
    let body = read(doc);
    let flat = flat_lower(&body);

    assert!(
        flat.contains("## commercial licensing"),
        "{doc} must carry a Commercial licensing section: a reader working out \
         whether they need to buy anything should not have to infer it from the \
         holder's identity"
    );

    for (required, why) in REQUIRED {
        assert!(
            flat.contains(required),
            "{doc}: {why} (missing `{required}`)"
        );
    }
    for (stale, why) in STALE {
        assert!(!flat.contains(stale), "{doc}: {why} (remove `{stale}`)");
    }

    assert!(
        body.contains(OWNER),
        "{doc} must name `{OWNER}` as the party able to grant a production \
         licence — naming the power without naming the holder is what sends a \
         reader to the wrong organization"
    );

    // No figure, in any of the shapes one arrives in — read over this section
    // rather than the whole document. A file-wide check is both too blunt and
    // too weak here: `$CARGO_HOME` in the release notes is not a price, and a
    // figure that landed in some other section would not be this guard's to
    // catch.
    let section = {
        let heading = body
            .find("## Commercial licensing")
            .expect("the heading was asserted above");
        let rest = &body[heading + "## Commercial licensing".len()..];
        let end = rest.find("\n## ").map_or(rest.len(), |at| at);
        rest[..end].to_lowercase()
    };
    assert!(
        !section.contains('$'),
        "the Commercial licensing section publishes no figure; a deployment is \
         quoted per engagement"
    );
    for shape in ["per seat", "per year", "starting at", "annual fee", "usd"] {
        assert!(
            !section.contains(shape),
            "`{shape}` reads as a price in the Commercial licensing section, \
             which quotes per engagement"
        );
    }
}

/// The chain of title is recorded, and no document in the tree contradicts it.
///
/// This test exists because of how its predecessor failed. Ownership used to be
/// checked by confirming that a hand-listed set of files agreed with a constant
/// — and they did agree, for months, on an answer that was wrong. Nothing had
/// conveyed the copyright to the organization a dozen files named. A guard that
/// asks "do these files say the same thing" cannot see a claim that is uniformly
/// false, which is the only kind of false claim a careful project actually
/// produces.
///
/// So the mechanism inverts. Instead of collecting agreement, it looks for
/// *contradiction*, across every document in the tree rather than a list this
/// file happened to keep in step — and it requires the chain itself to be
/// written down somewhere a reader can find, so that "who owns this" has an
/// answer with reasoning attached rather than an assertion repeated.
///
/// **There is no exemption any more, and that is the point.** This guard used to
/// skip one workshop deck, because a deck is a script somebody reads aloud and
/// its wording belongs to its author. The skip was written to cancel itself: it
/// asserted the deck *still carried* the stale claim, so fixing the deck would
/// fail the test and force the exemption's deletion. The deck was fixed; the
/// exemption is deleted; every Markdown document in the tree is now swept the
/// same way. An exemption kept past its reason is how a file stops being checked
/// at all.
#[test]
fn the_chain_of_title_is_recorded_and_nothing_contradicts_it() {
    /// Where the chain is written down.
    const RECORD: &str = "docs/licensing.md";

    /// Ways a document says the Foundation owns the copyright.
    ///
    /// Matched against prose with its whitespace collapsed, its case folded,
    /// **and its emphasis markers stripped**. That last one is not a nicety: the
    /// stale claim was written `copyright the **Neon Law Foundation**` in
    /// several files, so a matcher that reads asterisks as characters misses the
    /// exact formatting the tree actually used. A guard whose pattern is
    /// defeated by bold text is a guard that would have passed through the
    /// entire period this was wrong.
    const CONTRADICTIONS: [&str; 4] = [
        "copyright the neon law foundation",
        "copyright neon law foundation",
        "neon law foundation, which produces it; the firm operates it",
        "neon law foundation produces the software and holds the copyright",
    ];

    // ---- The chain is recorded, with reasoning rather than an assertion. ----
    let record = read(RECORD);
    let flat_record = unemphasized(&record);

    assert!(
        flat_record.contains("## chain of title"),
        "{RECORD} must carry a Chain of title section. `{OWNER} owns this`          repeated in a dozen files is what the previous version of this guard          confirmed, and it was wrong the whole time; a reader needs the route,          not the conclusion"
    );
    for required in [
        "17 u.s.c. § 204(a)",
        "licence to publish",
        "not yet filed",
        "not yet recorded",
    ] {
        assert!(
            flat_record.contains(required),
            "{RECORD}'s Chain of title must record `{required}` — an instrument              nobody can identify, or a registration step whose status is              unstated, is a gap that reads as a completed chain"
        );
    }
    assert!(
        record.contains(OWNER),
        "{RECORD} must name `{OWNER}` as the holder — the chain's whole point is \
         that a reader can follow it to a party who can actually grant this work"
    );

    // ---- Nothing in the tree says otherwise. ----
    let mut offenders = Vec::new();

    for path in markdown_documents() {
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace("../", "");
        let flat = unemphasized(&fs::read_to_string(&path).unwrap_or_default());
        let stale: Vec<&str> = CONTRADICTIONS
            .into_iter()
            .filter(|claim| flat.contains(claim))
            .collect();

        for claim in stale {
            offenders.push(format!("{rel}: says `{claim}`"));
        }
    }

    assert!(
        offenders.is_empty(),
        "the copyright belongs to {OWNER}; these documents say otherwise, and          agreeing with each other is exactly how the wrong answer survived          before:\n  {}",
        offenders.join("\n  ")
    );
}

/// The trademark reservation restates the licence's own carve-out and adds
/// nothing.
///
/// BUSL already declines to grant trademark rights: "This License does not grant
/// you any right in any trademark or logo of Licensor." So unlike the AGPL — whose
/// text is silent on marks, and where the reservation had to be introduced as a
/// § 7(e) additional term to exist at all — there is nothing to add here, and
/// adding something would violate BUSL's fourth covenant against modifying the
/// licence in any other way.
///
/// That makes this guard's job the opposite of its predecessor's. It is no longer
/// checking that an added term names its authority and disclaims being a further
/// restriction; it is checking that `NOTICE` explains *whose* marks the licence's
/// own clause is talking about, and that no § 7(e) machinery survives the
/// relicence in either file. A § 7(e) term under BUSL cites a section that does
/// not exist.
#[test]
fn the_trademark_reservation_restates_the_licences_own_carve_out() {
    /// What the reservation has to establish, and what each part prevents.
    const REQUIRED: [(&str, &str); 4] = [
        (
            "rights in copyright, not in trademarks",
            "the reservation must say what the licence does and does not reach; \
             the licence's own clause names no mark and no owner",
        ),
        (
            "nominative reference",
            "a fork has to be able to say truthfully what it is based on, or the \
             reservation reads as reaching past trademark law",
        ),
        (
            "may not present your deployment as neon law",
            "the operative sentence must name the thing that is actually \
             forbidden, which is passing a deployment off as the practice",
        ),
        (
            "views::brand_bundle",
            "telling someone they may not use the marks without showing them the \
             rename seam leaves patching sources as the obvious move",
        ),
    ];

    let notice = read(NOTICE_FILE);
    let flat = flat_lower(&notice);

    for (required, why) in REQUIRED {
        assert!(
            flat.contains(required),
            "{NOTICE_FILE}: {why} (missing `{required}`)"
        );
    }

    assert!(
        notice.contains(REGISTRANT),
        "the reservation must name `{REGISTRANT}`, whose marks it describes — a \
         reservation that names no owner tells a reader nothing about whose \
         permission they would be asking for"
    );
    assert!(
        notice.contains("6,325,650"),
        "{NOTICE_FILE} must cite the NEON LAW registration it reserves"
    );

    // § 7(e) was the AGPL's mechanism for adding a term the licence text did not
    // contain. BUSL contains the carve-out itself and forbids modification, so a
    // surviving § 7(e) term cites a section of a licence this work is no longer
    // under.
    for file in [LICENSE_FILE, NOTICE_FILE] {
        let body = flat_lower(&read(file));
        assert!(
            !body.contains("section 7(e)") && !body.contains("§ 7(e)"),
            "{file} still carries a § 7(e) additional term. {LICENSE} has no \
             § 7, states the trademark carve-out itself, and its fourth covenant \
             forbids modifying it — so the term now cites nothing and reads as an \
             unauthorized addition."
        );
    }

    // The complement of `the_licence_file_is_the_grant_text_with_our_parameters`:
    // that test proves `LICENSE` opens and closes on the licence's own lines, and
    // this proves our prose did not find its way in between. `OWNER` is
    // deliberately absent from this list — BUSL's parameters block names the
    // Licensor, so the Firm's name belongs in `LICENSE` and only there.
    let license = flat_lower(&read(LICENSE_FILE));
    for stray in ["6,325,650", "nominative reference", "views::brand_bundle"] {
        assert!(
            !license.contains(stray),
            "{LICENSE_FILE} must stay the licence text plus its parameters; \
             `{stray}` belongs in {NOTICE_FILE}, where no licence scanner reads it"
        );
    }
}

/// Public surfaces that name the NEON LAW registration attribute it to the Firm.
///
/// U.S. Reg. No. 6,325,650 belongs to the Firm, and the Firm licenses it to the
/// nobody else. A trademark notice that names
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
        // The bundled terms file carries the citation for the served site.
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
