//! Guard `docs/oss-install.md` §0 against the drift that made the page
//! unusable: it is the first thing an outside operator reads, and every
//! prerequisite it omits is a command that fails on a clean machine with no
//! hint about what is missing.
//!
//! A prerequisite list maintained by hand drifts silently, because the person
//! adding a tool to `require_tools` has no reason to be reading the install
//! page. That is how §0 came to name neither `kind` nor `helm` while
//! `cli::devx::orchestrate` refused the cluster lane without both — a reader
//! could install everything the page asked for and still be turned away by a
//! tool it never mentioned.
//!
//! So derive the checks from the sources instead of restating them: the tool
//! list comes out of `orchestrate.rs`'s own `require_tools` calls, the pinned
//! versions out of the workflow that pins them, and every `path:line`
//! citation has to resolve to a line that still says what it is cited for.
//! A tool added to the lane, or a pin bumped, now fails here until the page
//! catches up.

use std::fs;
use std::path::PathBuf;

const OSS_INSTALL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/oss-install.md"
));

const ORCHESTRATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/devx/orchestrate.rs"
));

const DEPLOY_YML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../.github/workflows/deploy.yml"
));

/// The workspace root, which every citation on the page is relative to.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// `## 0. Prerequisites` through the next `## ` heading.
fn prerequisites_section() -> &'static str {
    let (_, section) = OSS_INSTALL
        .split_once("## 0. Prerequisites")
        .expect("oss-install.md must carry a `## 0. Prerequisites` section");
    section
        .split_once("\n## ")
        .map_or(section, |(section, _)| section)
}

/// Every tool name passed to a `require_tools(&[…])` call in `orchestrate.rs`
/// whose list mentions `kind` — i.e. the calls that open the KIND cluster
/// lane, as opposed to the `kubectl`-only helpers that assume it is already up.
fn cluster_lane_tools() -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    for (at, needle) in ORCHESTRATE.match_indices("require_tools(&[") {
        let rest = &ORCHESTRATE[at + needle.len()..];
        let list = rest
            .split_once("])")
            .expect("a require_tools call must close its slice literal")
            .0;
        let names: Vec<String> = list
            .split(',')
            .map(|name| name.trim().trim_matches('"').to_string())
            .filter(|name| !name.is_empty())
            .collect();
        if !names.iter().any(|name| name == "kind") {
            continue;
        }
        for name in names {
            if !tools.contains(&name) {
                tools.push(name);
            }
        }
    }
    assert!(
        !tools.is_empty(),
        "orchestrate.rs must open the cluster lane with require_tools"
    );
    tools
}

/// A reader who installs everything §0 lists must be able to start the local
/// cluster. `orchestrate.rs` decides that, so read the answer from there.
#[test]
fn prerequisites_name_every_tool_the_kind_cluster_lane_requires() {
    let section = prerequisites_section();
    for tool in cluster_lane_tools() {
        assert!(
            section.to_lowercase().contains(&tool.to_lowercase()),
            "`docs/oss-install.md` §0 must name `{tool}`: `cli/src/devx/orchestrate.rs` refuses to \
             start the KIND cluster lane without it, so a reader who installs only what §0 lists \
             is turned away by a tool the page never mentioned"
        );
    }
}

/// The `helm/kind-action` step in `deploy.yml` — its `with:` block pins both
/// the `kind` binary and the node image it must stay in lockstep with.
///
/// Anchored on `uses: helm/kind-action` rather than the bare action name: a
/// comment earlier in the job explains why the tool cache is recreated *for*
/// that action, and matching the prose would return a block with no pins in it.
fn kind_action_block() -> &'static str {
    let (_, after) = DEPLOY_YML
        .split_once("uses: helm/kind-action")
        .expect("deploy.yml must install kind through helm/kind-action");
    after
        .split_once("- uses:")
        .map_or(after, |(block, _)| block)
}

/// §0 tells the reader to take a specific `kind`, and the only reason that
/// version is the right one is that CI proves the stack green on it. Pin the
/// page to the workflow rather than to a copied string, so bumping the gate
/// cannot leave the install page recommending the version CI abandoned.
#[test]
fn prerequisites_name_the_kind_version_and_node_image_deploy_pins() {
    let block = kind_action_block();

    let version = block
        .lines()
        .find_map(|line| line.trim().strip_prefix("version: "))
        .expect("the kind action must pin a `version:`");
    let node_image = block
        .lines()
        .find_map(|line| line.trim().strip_prefix("node_image: kindest/node:"))
        .expect("the kind action must pin a `node_image:`");
    let node_tag = node_image
        .split_once('@')
        .map_or(node_image, |(tag, _)| tag);

    let section = prerequisites_section();
    assert!(
        section.contains(version),
        "`docs/oss-install.md` §0 must tell the reader to install kind {version}, the version \
         `.github/workflows/deploy.yml` pins for the KIND gate"
    );
    assert!(
        section.contains(node_tag),
        "`docs/oss-install.md` §0 must name node image {node_tag}: the page tells the reader the \
         kind binary and the node image are pinned in lockstep, so both halves have to be current"
    );
}

/// Inline `path/to/file.ext:123` citations on the page, as (path, line).
///
/// Trims the surrounding markdown punctuation rather than parsing code spans:
/// the page mixes inline backticks with fenced blocks, so backtick parity is
/// not a reliable delimiter, while the extension-plus-digits shape is.
fn citations(markdown: &str) -> Vec<(&str, usize)> {
    markdown
        .split_whitespace()
        // Leading `.` is load-bearing — `.github/workflows/...` — so only the
        // trailing side sheds sentence punctuation.
        .map(|word| word.trim_start_matches(|c: char| "`*_([<\"'".contains(c)))
        .map(|word| word.trim_end_matches(|c: char| "`*_.,;)]>\"'".contains(c)))
        .filter_map(|token| {
            let (path, line) = token.rsplit_once(':')?;
            // 1-indexed, so a `:0` is a malformed citation rather than a line.
            let line: usize = line.parse().ok().filter(|n| *n > 0)?;
            let (_, extension) = path.rsplit_once('.')?;
            matches!(extension, "rs" | "md" | "yml" | "yaml" | "toml" | "surql")
                .then_some((path, line))
        })
        .collect()
}

/// §0's whole repair was to stop asserting prerequisites and start citing the
/// code that demands them. A citation nobody checks decays into the same
/// unfounded claim, so hold every one of them to an existing line.
#[test]
fn every_source_citation_on_the_page_resolves() {
    let found = citations(OSS_INSTALL);
    assert!(
        found
            .iter()
            .any(|(path, _)| *path == "cli/src/devx/orchestrate.rs"),
        "the page must cite the code that gates the cluster lane"
    );

    let root = workspace_root();
    for (path, line) in found {
        let body = fs::read_to_string(root.join(path))
            .unwrap_or_else(|e| panic!("`docs/oss-install.md` cites `{path}:{line}`, but {e}"));
        let cited = body.lines().nth(line - 1).unwrap_or_else(|| {
            panic!(
                "`docs/oss-install.md` cites `{path}:{line}`, but that file has only {} lines",
                body.lines().count()
            )
        });
        assert!(
            !cited.trim().is_empty(),
            "`docs/oss-install.md` cites `{path}:{line}`, which is a blank line — the citation \
             has drifted off the code it was pointing at"
        );
    }
}

/// §0's citations are turned into instructions — install *this* version,
/// because *that* line demands it — so a line that merely exists is not
/// enough. Assert over the set of cited lines rather than pinning each
/// citation to a position, since the page is free to reorder or reword the
/// bullets as long as all four claims still land on their code.
#[test]
fn the_cluster_lane_citations_land_on_the_code_they_describe() {
    let root = workspace_root();
    let cited: Vec<String> = citations(prerequisites_section())
        .into_iter()
        .map(|(path, line)| {
            let body = fs::read_to_string(root.join(path))
                .unwrap_or_else(|e| panic!("§0 cites `{path}:{line}`, but {e}"));
            body.lines()
                .nth(line - 1)
                .unwrap_or_else(|| panic!("§0 cites `{path}:{line}`, which is past end of file"))
                .to_string()
        })
        .collect();

    for (claim, needle) in [
        ("the cluster lane's tool gate", "require_tools(&[\"kind\""),
        (
            "the one command helm is needed for",
            "Command::new(\"helm\")",
        ),
        ("the pinned kind binary", "version: v"),
        ("the node image it locks to", "node_image: kindest/node:"),
    ] {
        assert!(
            cited.iter().any(|line| line.contains(needle)),
            "§0 describes {claim}, so one of its `path:line` citations must land on a line \
             containing `{needle}` — none of them do, so the page is explaining code it is no \
             longer pointing at"
        );
    }
}
