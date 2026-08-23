//! Guard against a recurring image-build drift: the `views` crate bakes in
//! files that live OUTSIDE its own crate dir — `include_str!(concat!(
//! CARGO_MANIFEST_DIR, "/../docs/lsp/*.md"))` — so any Docker builder that
//! compiles `views` must stage the `docs` tree, or `cargo build` fails at
//! `couldn't read .../docs/lsp/README.md`. Every worker Containerfile stages the
//! whole workspace (including `views`) to satisfy Cargo's workspace member
//! resolution, so the checkable invariant is: if a Containerfile copies
//! `views`, it must also copy `docs`. This asserts exactly that across every
//! `images/Containerfile.*`, so a new image (or a new crate that pulls `views`
//! into an existing one) can't reintroduce the missing-`docs` build failure.

use std::fs;
use std::path::PathBuf;

/// The `images/` directory at the workspace root (this test crate is `cli`).
fn images_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("images")
}

/// `dx` writes the wasm client to `target/dx/webapp/<profile>/web/public`.
///
/// That `web` is Dioxus's **platform** directory, not a crate — so a sweep that
/// renames the `web` crate silently rewrites it, and nothing notices: the CLI
/// bails only when someone runs `dev build-webapp`, and the image copies an
/// empty directory into `/app/public/dioxus`. #860 corrupted exactly this.
///
/// So pin that `cli/src/devx/webapp.rs` and the host images name the same path.
#[test]
fn the_dx_bundle_path_agrees_between_the_cli_and_the_images() {
    const PLATFORM_DIR: &str = "web/public";

    let cli_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/devx/webapp.rs");
    let body =
        fs::read_to_string(&cli_src).unwrap_or_else(|e| panic!("read {}: {e}", cli_src.display()));
    assert!(
        body.contains(&format!("join(\"{PLATFORM_DIR}\")")),
        "{} must join dx's platform dir `{PLATFORM_DIR}` — it is Dioxus's own name, \
         not the `web` crate, so it does not follow a crate rename",
        cli_src.display()
    );

    for file in BRAND_IMAGES.map(|brand| format!("Containerfile.{brand}")) {
        let path = images_dir().join(&file);
        let image =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert!(
            image.contains(&format!("target/dx/webapp/release/{PLATFORM_DIR}")),
            "{} must stage the dx bundle from `target/dx/webapp/release/{PLATFORM_DIR}`, \
             the same path `cli::devx::webapp` writes it to",
            path.display()
        );
    }
}

/// Every release wasm client build must turn dx's `--debug-symbols` OFF.
///
/// That flag defaults to **true** in dioxus-cli, `--release` included, and it is
/// the single input that decides whether wasm-opt is invoked with `--debuginfo`
/// or with `--strip-debug`. Left at its default, three things follow in order:
/// dx's ad-hoc `wasm-release` profile sets `strip=false`, so ~340 KB of DWARF
/// survives into the module; wasm-opt is then asked to re-emit that DWARF and
/// binaryen aborts doing it (`UNREACHABLE executed at DWARFEmitter.cpp:201`,
/// SIGABRT, core dumped); and dioxus-cli, which treats a failed wasm-opt as
/// non-fatal, copies the UNOPTIMIZED module through as the bundle.
///
/// So the whole failure is silent. `build navigator-web` stayed green through
/// every deploy while publishing a 941 KB `webapp_bg.wasm` in place of the
/// 534 KB one `-Oz --strip-debug` produces — a 43% tax on every visitor who
/// hydrates a page, paid for months because an ERROR line in a 6,000-line image
/// log is not a check.
///
/// Nothing else can catch this: the workspace suite never builds a bundle, and
/// the wasm lane in `ci.yml` is a `cargo check`, which does not reach wasm-opt.
/// So assert the flag itself, in all three places that spell the build out.
#[test]
fn the_release_wasm_client_build_strips_debug_symbols() {
    const FLAG: &str = "--debug-symbols false";

    let cli_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/devx/webapp.rs");
    let body =
        fs::read_to_string(&cli_src).unwrap_or_else(|e| panic!("read {}: {e}", cli_src.display()));
    assert!(
        body.contains(r#".arg("--debug-symbols").arg("false")"#),
        "{} must pass `{FLAG}` to `dx build --release`: dx defaults it to true, \
         which makes wasm-opt rewrite DWARF instead of stripping it, abort, and \
         leave dioxus-cli to ship the unoptimized module",
        cli_src.display()
    );

    for file in BRAND_IMAGES.map(|brand| format!("Containerfile.{brand}")) {
        let path = images_dir().join(&file);
        let image =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        // Anchor on the RUN line, not any line mentioning the command: the
        // comment block above it explains dx's tool staging and says `dx build`
        // in prose.
        let dx_build = image
            .lines()
            .find(|line| line.starts_with("RUN") && line.contains("dx build"))
            .unwrap_or_else(|| panic!("{} has no `RUN … dx build` line", path.display()));
        assert!(
            dx_build.contains(FLAG),
            "{} must build the release bundle with `{FLAG}`, or it publishes an \
             unoptimized wasm module carrying DWARF. Found: {dx_build}",
            path.display()
        );
        assert!(
            image.contains(".debug_info"),
            "{} must also assert no `.debug_info` section survives into the staged \
             bundle. dioxus-cli swallows a wasm-opt failure, so the flag alone is \
             not proof — the image build is the only place that can check the bytes",
            path.display()
        );
    }
}

/// True when the Containerfile has a `COPY <src> <crate>` line staging the
/// named workspace crate into the build context.
fn copies_crate(body: &str, crate_name: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim();
        if !line.starts_with("COPY ") {
            return false;
        }
        // `COPY views views` / `COPY docs docs` — the crate name is a
        // whitespace-delimited token on the line (source and/or dest).
        line.split_whitespace().skip(1).any(|tok| tok == crate_name)
    })
}

#[test]
fn every_containerfile_that_copies_views_also_copies_docs() {
    let dir = images_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));

    let mut checked = 0;
    let mut offenders = Vec::new();
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("Containerfile") {
            continue;
        }
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        checked += 1;
        if copies_crate(&body, "views") && !copies_crate(&body, "docs") {
            offenders.push(name);
        }
    }

    assert!(checked > 0, "no Containerfiles found in {}", dir.display());
    assert!(
        offenders.is_empty(),
        "these Containerfiles stage the `views` crate but not `docs`, so a \
         build that compiles `views` fails on its include_str! of \
         `docs/lsp/*.md`; add `COPY docs docs`: {offenders:?}"
    );
}

/// The site image publishes its content from disk, so it must stage the bundled
/// content tree and point the binary at it.
///
/// Only the roots the binary actually reads are asserted. `NAVIGATOR_MARKETING_DIR`
/// and `NAVIGATOR_FOUNDATION_DIR` used to sit beside these and are gone: both
/// named directories that went with the Foundation surface, and nothing in the
/// workspace ever read either variable — so they were dead config pointing at
/// paths absent from the image.
#[test]
fn the_site_image_maps_bundled_content_to_its_runtime_dir() {
    let path = images_dir().join("Containerfile.neon");
    let body = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    assert!(
        body.contains("COPY --from=builder /src/server/content /app/content"),
        "{} must copy the bundled site content into the runtime image",
        path.display()
    );
    for root in [
        "NAVIGATOR_BLOG_DIR=/app/content/blog",
        "NAVIGATOR_WORKSHOPS_DIR=/app/content/workshops",
    ] {
        assert!(
            body.contains(root),
            "{} must point the binary at its bundled content ({root})",
            path.display()
        );
    }
    // A content root naming a directory the tree no longer carries is worse
    // than none: the image sets it, nothing reads it, and a reader takes it as
    // evidence the content ships.
    for retired in ["NAVIGATOR_FOUNDATION_DIR", "NAVIGATOR_MARKETING_DIR"] {
        assert!(
            !body.contains(retired),
            "{} still sets {retired}, which names a directory this tree deleted",
            path.display()
        );
    }
}

/// The workspace root (this test crate is `cli`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The repo-relative `COPY` sources in a Containerfile.
///
/// Skips what cannot be checked against the working tree: `--from=<stage>`
/// copies (their sources are paths inside an earlier build stage, not the
/// repo), the whole-context `COPY . ./`, build-arg interpolations, globs, and
/// absolute paths. What remains is exactly the set of workspace paths the
/// build context must actually contain.
///
/// Sources are anchored on the `COPY ` prefix rather than matched as a bare
/// substring: every brand Containerfile stages
/// `target/dx/webapp/release/web/public`, which is Dioxus's own platform
/// directory and has never been the `web` crate.
fn repo_relative_copy_sources(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let Some(rest) = line.trim().strip_prefix("COPY ") else {
            continue;
        };
        let toks: Vec<&str> = rest.split_whitespace().collect();
        // `COPY --from=builder /usr/local/bin/neon /app/neon` reads out of
        // an earlier stage's filesystem, which this test cannot see.
        if toks.iter().any(|tok| tok.starts_with("--")) {
            continue;
        }
        // The last token is the destination; every token before it is a source,
        // so `COPY Cargo.toml Cargo.lock rust-toolchain.toml ./` checks three.
        let Some((_dest, sources)) = toks.split_last() else {
            continue;
        };
        for src in sources {
            let src = src.trim_matches('"');
            let uncheckable = src == "."
                || src.starts_with('/')
                || src.contains('$')
                || src.contains('*')
                || src.contains('?');
            if !uncheckable {
                out.push(src.to_string());
            }
        }
    }
    out
}

/// No Containerfile may stage a path that is not in the workspace.
///
/// The sibling guards above check the two directions #860 already broke: a
/// workspace member missing from a COPY list, and a `cargo build -p <crate>`
/// naming a crate that no longer exists. Neither catches the inverse — a COPY
/// line left pointing at a *deleted* directory. #906 deleted the two host crates
/// of the day and left their `COPY` lines behind in all seven
/// images, which every PR check happily ignored (`cargo test (workspace)` never
/// builds an image) while the release deploy died at
/// `failed to compute cache key: "/web": not found` — for two days, publishing
/// nothing.
///
/// So assert the invariant directly: every plain repo-relative COPY source
/// resolves in the tree. This makes deleting or renaming a directory fail here,
/// in the required workspace test, instead of at image-publish time.
#[test]
fn no_containerfile_copies_a_path_that_no_longer_exists() {
    let root = workspace_root();
    let dir = images_dir();

    let mut checked = 0;
    let mut offenders = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("Containerfile") {
            continue;
        }
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for src in repo_relative_copy_sources(&body) {
            checked += 1;
            if !root.join(&src).exists() {
                offenders.push(format!("{name} copies `{src}`"));
            }
        }
    }

    assert!(
        checked > 0,
        "no repo-relative COPY sources found in {}",
        dir.display()
    );
    assert!(
        offenders.is_empty(),
        "these Containerfiles stage a path that is not in the workspace, so `docker build` fails \
         with `failed to compute cache key: not found` in the release deploy while every PR check \
         stays green; delete the stale COPY line: {offenders:?}"
    );
}

/// The workspace members declared in the root `Cargo.toml`.
fn workspace_members() -> Vec<String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("Cargo.toml");
    let body = fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    let members = body
        .split_once("members = [")
        .expect("root Cargo.toml declares `members = [`")
        .1
        .split_once(']')
        .expect("the `members` array is closed")
        .0;
    members
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',').trim();
            let name = line.strip_prefix('"')?.strip_suffix('"')?;
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// Every worker Containerfile stages the whole workspace so Cargo can resolve
/// its members; a member listed in the root `Cargo.toml` but absent from the
/// build context fails `cargo build` with "failed to load manifest for
/// workspace member" — at image-build time in CI, not locally.
///
/// This reads the member list rather than pinning one crate by name, so adding
/// a workspace member cannot silently break the image builds. It runs in
/// `cargo test (workspace)`, which the image builds are not.
///
/// It also *discovers* the Containerfiles rather than listing them, and is the
/// only guard for this invariant. Discovery covers strictly more than a
/// hardcoded list and cannot rot as images come and go.
#[test]
fn every_workspace_staging_containerfile_copies_every_member() {
    let dir = images_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    let members = workspace_members();
    assert!(
        members.iter().any(|m| m == "server"),
        "expected `server` among the workspace members, got {members:?}"
    );

    let mut checked = 0;
    let mut offenders = Vec::new();
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("Containerfile") {
            continue;
        }
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        // A Containerfile that copies `portal` stages the workspace to build a
        // Rust binary; it must therefore copy every member.
        if copies_crate(&body, "portal") {
            checked += 1;
            let missing: Vec<&str> = members
                .iter()
                .map(String::as_str)
                .filter(|member| !copies_crate(&body, member))
                .collect();
            if !missing.is_empty() {
                offenders.push(format!("{name} (missing: {})", missing.join(", ")));
            }
        }
    }

    assert!(checked > 0, "no workspace-staging Containerfiles found");
    assert!(
        offenders.is_empty(),
        "these Containerfiles stage the workspace but omit a member, so \
         `cargo build` fails to load its manifest; add the matching `COPY <member> <member>`: \
         {offenders:?}"
    );
}

/// No Containerfile may build or run a crate that is not a workspace member.
///
/// `cargo test (workspace)` never builds an image, so a Containerfile
/// pointing at a crate the workspace does not have stays green here and fails
/// the release deploy instead. The COPY-list guard above reads only staged
/// directories, so this is the check that reads the build targets themselves.
#[test]
fn no_containerfile_builds_a_crate_that_no_longer_exists() {
    let members = workspace_members();
    let dir = images_dir();
    let build = regex_lite_build_targets;

    let mut checked = 0;
    let mut offenders = Vec::new();
    for entry in fs::read_dir(&dir).expect("read images dir") {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("Containerfile") {
            continue;
        }
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for target in build(&body) {
            checked += 1;
            if !members.contains(&target) {
                offenders.push(format!("{name} builds `-p {target}`"));
            }
        }
    }

    assert!(checked > 0, "no `cargo build -p <crate>` lines found");
    assert!(
        offenders.is_empty(),
        "these Containerfiles build a crate that is not a workspace member, so the image build \
         fails in the release deploy while every PR check stays green: {offenders:?}"
    );
}

/// Every `-p <crate>` argument on a `cargo build` line in a Containerfile.
fn regex_lite_build_targets(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.contains("cargo build") {
            continue;
        }
        let mut toks = line.split_whitespace();
        while let Some(tok) = toks.next() {
            if tok == "-p" {
                if let Some(target) = toks.next() {
                    let target = target.trim_matches('"');
                    // `Containerfile.trigger` parameterises its target with a
                    // build arg (`-p ${CRATE}`); only literal names are
                    // checkable against the member list.
                    if !target.contains('$') {
                        out.push(target.to_string());
                    }
                }
            }
        }
    }
    out
}

/// The brand images this workspace publishes.
///
/// One: `neon` serves the firm at the site root and the Foundation beneath
/// `/foundation`, so there is nothing for a second image to be. It stays a
/// list rather than a constant because the white-label tenant shape
/// (`portal::tenant`) is a second brand waiting to happen, and the guards
/// below are written to hold whenever it arrives.
const BRAND_IMAGES: [&str; 1] = ["neon"];

/// Each brand image builds its own brand binary and runs it with no flag.
///
/// The binary *is* the site, so the defect this guards is a `Containerfile`
/// that builds one crate and publishes another under `neon-server` — an image
/// serving the wrong legal entity. Pin the whole chain: the crate built, the
/// binary staged, and the entrypoint that runs it.
#[test]
fn each_brand_image_builds_and_entrypoints_its_own_brand_binary() {
    for brand in BRAND_IMAGES {
        let path = images_dir().join(format!("Containerfile.{brand}"));
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        assert!(
            body.contains(&format!(
                "cargo build --release --target \"$(cat /tmp/rust-target)\" -p {brand}"
            )),
            "{} must build the `{brand}` brand binary",
            path.display()
        );
        assert!(
            body.contains(&format!(
                "COPY --from=builder /usr/local/bin/{brand} /app/{brand}"
            )),
            "{} must stage the `{brand}` binary into the runtime image",
            path.display()
        );
        assert!(
            body.contains(&format!("ENTRYPOINT [\"/app/{brand}\"]")),
            "{} must run `/app/{brand}` — the binary is the site, so there is no flag",
            path.display()
        );
        assert!(
            !body.contains("--site"),
            "{} must not pass `--site`; the brand binaries do not accept it",
            path.display()
        );
        for other in BRAND_IMAGES.into_iter().filter(|b| *b != brand) {
            assert!(
                !body.contains(&format!("ENTRYPOINT [\"/app/{other}\"]")),
                "{} must not serve the `{other}` brand",
                path.display()
            );
        }
    }
}

/// The version of a crate as resolved in the workspace `Cargo.lock`.
///
/// Matches the `name = "<crate>"` line exactly so `wasm-bindgen` does not
/// collide with `wasm-bindgen-backend`, `-macro`, or `-shared`.
fn locked_crate_version(crate_name: &str) -> String {
    let lock = workspace_root().join("Cargo.lock");
    let body = fs::read_to_string(&lock).unwrap_or_else(|e| panic!("read {}: {e}", lock.display()));

    let mut lines = body.lines();
    while let Some(line) = lines.next() {
        if line.trim() != format!("name = \"{crate_name}\"") {
            continue;
        }
        for next in lines.by_ref() {
            if let Some(version) = next.trim().strip_prefix("version = ") {
                return version.trim_matches('"').to_string();
            }
            // A `[[package]]` boundary before any version means a malformed lock.
            if next.trim().starts_with('[') {
                break;
            }
        }
        panic!("{} has no version for `{crate_name}`", lock.display());
    }
    panic!("{} does not lock `{crate_name}`", lock.display());
}

/// The Containerfiles that shell `dx build` — the wasm-bundling host images.
fn containerfiles_running_dx_build() -> Vec<(String, String)> {
    let dir = images_dir();
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("Containerfile") {
            continue;
        }
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if body.contains("dx build") {
            out.push((name, body));
        }
    }
    assert!(
        !out.is_empty(),
        "no image runs `dx build` — this guard has lost its subject"
    );
    out
}

/// `dx build` must not reach the network for its own tooling.
///
/// `dx` resolves three binaries while building a web bundle — wasm-bindgen and
/// esbuild in `verify_web_tooling`, wasm-opt in `bundle_web` — and by default
/// downloads each one on the spot. In an image build that spot is the *last*
/// layer: it depends on the whole workspace, so it never caches and re-fetches
/// on every single build, unretried. One reset mid-body ("Failed to create
/// wasm-opt instance … connection reset") failed `publish navigator-web` and
/// published nothing that day (#935).
///
/// The fix stages all three at pinned versions in an early, cacheable layer and
/// passes `NO_DOWNLOADS=1`, which switches dioxus-cli's lookup from "download
/// into a cache dir" to `which`. Assert that wiring here so a new host image —
/// or a well-meaning cleanup of the "redundant" pins — cannot quietly reopen
/// the fetch.
#[test]
fn every_dx_build_takes_its_tooling_from_pinned_binaries_not_the_network() {
    for (name, body) in containerfiles_running_dx_build() {
        assert!(
            body.contains("NO_DOWNLOADS=1 dx build"),
            "images/{name} must invoke `dx build` with `NO_DOWNLOADS=1` so dioxus-cli \
             resolves wasm-bindgen, esbuild, and wasm-opt from PATH instead of \
             downloading them in the final, never-cached layer (#935)"
        );
        for arg in [
            "ARG WASM_BINDGEN_VERSION=",
            "ARG BINARYEN_VERSION=",
            "ARG ESBUILD_VERSION=",
        ] {
            assert!(
                body.contains(arg),
                "images/{name} must pin its dx tooling with `{arg}…` — an unpinned \
                 tool is a network fetch inside the build"
            );
        }
        // The staged binaries are only reachable if their prefixes are on PATH;
        // `wasm-bindgen` lands in $CARGO_HOME/bin, which already is.
        for prefix in ["/opt/binaryen/bin", "/opt/esbuild/bin"] {
            assert!(
                body.contains(prefix),
                "images/{name} must put `{prefix}` on PATH so `NO_DOWNLOADS=1` can \
                 resolve the pinned binary"
            );
        }
    }
}

/// The pinned wasm-bindgen-cli must match the locked `wasm-bindgen` crate.
///
/// `dx` reads the expected version out of the resolved crate graph and then
/// runs `wasm-bindgen --version`, rejecting any mismatch outright:
/// "project requires version X but version Y is installed". So a routine
/// dependency bump of the `wasm-bindgen` *crate* breaks the image build unless
/// the pinned *CLI* moves with it — and because no PR check builds an image,
/// that break would first surface in the release deploy. Catch it in the
/// required workspace test instead.
#[test]
fn the_pinned_wasm_bindgen_cli_matches_the_locked_wasm_bindgen_crate() {
    let locked = locked_crate_version("wasm-bindgen");

    for (name, body) in containerfiles_running_dx_build() {
        let pinned = body
            .lines()
            .find_map(|line| line.trim().strip_prefix("ARG WASM_BINDGEN_VERSION="))
            .unwrap_or_else(|| panic!("images/{name} has no ARG WASM_BINDGEN_VERSION"))
            .trim()
            .to_string();

        assert_eq!(
            pinned, locked,
            "images/{name} pins wasm-bindgen-cli {pinned} but Cargo.lock resolves the \
             `wasm-bindgen` crate to {locked}. `dx` compares the two and refuses to \
             build on a mismatch, so move the ARG with the dependency bump."
        );
    }
}

/// A Containerfile that compiles `cli` must stage the root files `cli` bakes in.
///
/// The same class of drift as the `views`/`docs` invariant above, one crate
/// over: `cli/src/main.rs` carries `include_str!("../../LICENSE")`,
/// `include_str!("../../NOTICE")`, and
/// `include_str!("../../THIRD-PARTY-NOTICES.txt")` so a downloaded binary can
/// print its own terms and attributions with no accompanying files. All three
/// live at the workspace root, outside the crate directory, so any builder that
/// stages `cli` without them fails at `couldn't read .../LICENSE` — and because
/// no PR check builds an image, the break would first surface in the release
/// deploy.
#[test]
fn images_that_copy_cli_also_copy_the_root_files_it_embeds() {
    let embedded = ["LICENSE", "NOTICE", "THIRD-PARTY-NOTICES.txt"];

    // Pin the reason this test exists: if the embeds move or are removed, this
    // fails first and points at the guard rather than at eight image builds.
    let main_rs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let main_body =
        fs::read_to_string(&main_rs).unwrap_or_else(|e| panic!("read {}: {e}", main_rs.display()));
    for file in embedded {
        assert!(
            main_body.contains(&format!("include_str!(\"../../{file}\")")),
            "cli/src/main.rs no longer embeds `{file}`; if that is deliberate, drop it \
             from this guard and from the images' COPY lists"
        );
    }

    let mut offenders = Vec::new();
    let entries = fs::read_dir(images_dir()).expect("read images/");
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("Containerfile.") {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        if !body.lines().any(|l| l.trim_start().starts_with("COPY cli")) {
            continue;
        }
        for file in embedded {
            let copied = body
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("COPY {file}")));
            if !copied {
                offenders.push(format!("images/{name}: copies `cli` but not `{file}`"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "`cli` embeds workspace-root files with include_str!, so every image that \
         stages `cli` must stage them too:\n  {}",
        offenders.join("\n  ")
    );
}
