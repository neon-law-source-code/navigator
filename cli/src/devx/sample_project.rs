//! `navigator dev sample-project` — clone, build, and stage each sample
//! matter's project application.
//!
//! Every sample matter carries a client portal at
//! `/app/projects/{code}/portal/`. Development boot clones the repository
//! recorded on each of those Projects, builds it with `pnpm`, and stages the
//! resulting `dist/` before writing the environment that `web` reads.
//!
//! The URLs come from the Projects rather than constants here, so pointing a
//! matter at a different forge is a data change. `--repo` still overrides one
//! for a fork or a local mirror, and `--project` narrows the refresh to a
//! single matter — the fast loop while iterating on one app, since a full
//! refresh is one `pnpm install` and build per matter.
//!
//! The clone and the build happen in a **temporary directory** that is removed
//! when the command returns — a build tree is derived, so keeping it in the
//! worktree would only invite editing the wrong copy. Two things survive into
//! `.devx/sample-projects/<code>/`: the built `dist/`, and the `navigator.yaml`
//! that declares which Project the bundle mounts on. Boot re-reads that
//! manifest rather than trusting the directory it was found in, so the pair
//! travels together and a bundle staged under the wrong code is refused rather
//! than published on another matter's portal. `--keep` retains the temp tree
//! for debugging a failed build.
//!
//! Nothing here runs in production: only the local boot path stages it.
//!
//! ## Testing
//!
//! Cloning and `pnpm` shell out to the network and to Node, so [`run`]'s
//! sequencing is not unit-tested. Everything that *decides* something is
//! extracted so it can be: which matters to refresh ([`choose_matters`]),
//! which repository to clone ([`choose_repo`]), the git arguments, the two
//! preconditions a contributor actually trips ([`require_lockfile`],
//! [`built_bundle`]), the staged paths, and the tree copy. What is left in
//! `run` is the order of the shell-outs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Where the projects are staged, relative to the workspace root. Inside
/// `.devx/` because it is generated, per-checkout, and already ignored. Each
/// matter gets a subdirectory named for its Project code.
const STAGE_RELATIVE: [&str; 1] = ["sample-projects"];

/// Build the `git clone` arguments. Always a shallow, single-branch clone: the
/// history is not wanted, only the tree that builds.
///
/// A pinned `git_ref` still clones shallow — `--branch` accepts a tag or a
/// branch name — so the common case stays one round trip.
fn clone_args(repo: &str, git_ref: Option<&str>, dest: &Path) -> Vec<String> {
    let mut args = vec![
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--single-branch".to_string(),
    ];
    if let Some(reference) = git_ref {
        args.push("--branch".to_string());
        args.push(reference.to_string());
    }
    args.push(repo.to_string());
    args.push(dest.display().to_string());
    args
}

/// The directory every staged project sits under — the one path `.devx/env`
/// names, so `web` finds all of them from a single variable.
fn staged_root(workspace_root: &Path) -> PathBuf {
    let mut path = workspace_root.join(".devx");
    for segment in STAGE_RELATIVE {
        path.push(segment);
    }
    path
}

/// Where one matter is staged for the next `web` boot — the manifest and the
/// built bundle together, under the matter's own Project code.
fn staged_for(workspace_root: &Path, project_code: &str) -> PathBuf {
    staged_root(workspace_root).join(project_code)
}

/// Copy a directory tree, replacing `dst` wholesale.
///
/// Replacing rather than merging is deliberate: a merge would leave assets
/// from a previous build in the staged tree, and boot publishes everything it
/// finds, so stale files would be republished forever.
fn copy_tree(src: &Path, dst: &Path) -> Result<usize> {
    if dst.exists() {
        std::fs::remove_dir_all(dst)
            .with_context(|| format!("clearing the staged bundle at {}", dst.display()))?;
    }
    let mut copied = 0;
    copy_into(src, dst, &mut copied)?;
    Ok(copied)
}

fn copy_into(src: &Path, dst: &Path, copied: &mut usize) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let metadata = std::fs::metadata(&from)?;
        if metadata.is_dir() {
            copy_into(&from, &to, copied)?;
        } else if metadata.is_file() {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
            *copied += 1;
        }
    }
    Ok(())
}

/// Run one command in `dir`, failing loudly. Output is inherited so a `pnpm`
/// build's own diagnostics reach the operator instead of being swallowed into
/// a captured buffer nobody prints.
fn run_in(dir: &Path, program: &str, args: &[String]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .with_context(|| format!("running `{program}` — is it installed and on PATH?"))?;
    if !status.success() {
        bail!(
            "`{program} {}` failed in {} ({status})",
            args.join(" "),
            dir.display()
        );
    }
    Ok(())
}

/// Whether the store was even consulted, so the repository choice can say
/// which of the two absences it is looking at.
enum Lookup {
    /// No row for this Project code at all.
    NoProject,
    /// The row exists and carries this `repository_url`.
    Project(Option<String>),
}

/// Which matters this invocation refreshes.
///
/// No `--project` refreshes all of them, which is what a boot wants. Naming
/// one narrows it to that matter, and a name that is not a sample matter is
/// refused with the list rather than silently refreshing nothing — a typo
/// there would otherwise look like a successful no-op.
fn choose_matters<'a>(project: Option<&str>, known: &[&'a str]) -> Result<Vec<&'a str>> {
    let Some(code) = project else {
        return Ok(known.to_vec());
    };
    match known.iter().find(|known| **known == code) {
        Some(found) => Ok(vec![*found]),
        None => bail!(
            "`{code}` is not a sample matter. Known matters: {}.",
            known.join(", ")
        ),
    }
}

/// Choose the repository to clone for one matter, given the flag and what the
/// store holds.
///
/// The pure half of [`resolve_repo`]: every branch a caller can land in is
/// decided here, so the IO wrapper stays a connect-and-read with no decisions
/// of its own.
///
/// `--repo` wins without a lookup. Otherwise the Project row wins, which is
/// what keeps pointing a matter at a different forge a data change. Only when
/// there is no row at all does the compiled-in URL for that matter stand in —
/// that is a store nobody has seeded yet, and the row the seed is about to
/// write carries exactly this URL. A row that exists but records no URL is
/// still an error, because something deliberately cleared it.
fn choose_repo(
    project_code: &str,
    explicit: Option<&str>,
    fallback: Option<&str>,
    lookup: impl FnOnce() -> Result<Lookup>,
) -> Result<String> {
    if let Some(repo) = explicit {
        return Ok(repo.to_string());
    }
    match lookup()? {
        Lookup::Project(Some(url)) => Ok(url),
        Lookup::Project(None) => bail!(
            "Project `{project_code}` records no repository URL. Set one on the matter, or \
             pass `--repo`."
        ),
        Lookup::NoProject => match fallback {
            Some(url) => Ok(url.to_string()),
            None => bail!(
                "no Project `{project_code}` in this store — start `web` once so the seed \
                 runs, or pass `--repo`"
            ),
        },
    }
}

/// The repository to clone for one matter: `--repo` when given, else the URL
/// recorded on the Project, else the compiled-in default for that matter.
///
/// Reading the Project is what keeps one source of truth. The decision lives in
/// [`choose_repo`]; this only supplies the store.
fn resolve_repo(project_code: &str, explicit: Option<&str>) -> Result<String> {
    let fallback = store::seed::sample_matter_repository(project_code);
    choose_repo(project_code, explicit, fallback, || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("create tokio runtime")?;
        runtime.block_on(async {
            let surreal = store::surreal::connect_from_env().await.context(
                "connect to SurrealDB to read the Project's repository URL — source \
                 this worktree's `.devx/env` first, or pass `--repo`",
            )?;
            Ok(
                match store::projects::find_by_code(&surreal, project_code)
                    .await
                    .with_context(|| format!("look up Project `{project_code}`"))?
                {
                    None => Lookup::NoProject,
                    Some(project) => Lookup::Project(project.repository_url),
                },
            )
        })
    })
}

/// The manifest text and the Project code it declares.
///
/// Read *before* a build is spent on the checkout: a bundle declaring the wrong
/// Project is refused at boot anyway, so finding out here saves an install. The
/// text is returned alongside the code because it is staged verbatim next to the
/// bundle — boot re-reads it rather than trusting whoever staged it.
fn declared_project(checkout: &Path) -> Result<(String, String)> {
    let manifest_path = checkout.join(store::sample_project::MANIFEST_FILE);
    let manifest = std::fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "reading {} — a project application declares its Project there",
            manifest_path.display()
        )
    })?;
    let code = store::sample_project::project_code_from_manifest(&manifest)?;
    Ok((manifest, code))
}

/// Refuse a checkout with no lockfile, before spending an install on it.
///
/// `--frozen-lockfile` is what keeps the build reproducible, so this says
/// plainly what is wrong rather than letting `pnpm` fail with its own less
/// specific message about a missing lockfile it was told not to write.
fn require_lockfile(checkout: &Path, repo: &str) -> Result<()> {
    if checkout.join("pnpm-lock.yaml").is_file() {
        return Ok(());
    }
    bail!(
        "{repo} has no pnpm-lock.yaml, so its dependencies cannot be resolved \
         reproducibly. Commit a lockfile there (which needs every dependency \
         to be resolvable — see its README) and re-run."
    )
}

/// The built bundle inside a checkout, proven to be one.
///
/// Both absences are a failed build rather than a partial one, and they are
/// reported separately because they have different causes: no `dist/` means the
/// build script did not run or writes elsewhere, while a `dist/` with no entry
/// document means it ran and produced assets nothing can point at. Publishing
/// the latter would strand the live bundle.
fn built_bundle(checkout: &Path) -> Result<PathBuf> {
    let built = checkout.join(store::sample_project::DIST_DIR);
    if !built.is_dir() {
        bail!(
            "the build produced no `dist/` at {} — check the repository's build script",
            built.display()
        );
    }
    if !built.join(store::sample_project::ENTRY_DOCUMENT).is_file() {
        bail!(
            "the build produced no `{}` — Navigator publishes nothing without an entry document",
            store::sample_project::ENTRY_DOCUMENT
        );
    }
    Ok(built)
}

/// Refresh the sample matters' applications for a local boot.
pub fn run(
    project: Option<&str>,
    repo: Option<&str>,
    git_ref: Option<&str>,
    keep: bool,
) -> Result<()> {
    super::require_tools(&["git", "pnpm"])?;
    let known = store::seed::sample_matter_codes();
    let matters = choose_matters(project, &known)?;
    let workspace_root = super::orchestrate::workspace_root()?;

    let mut copied = 0;
    for code in &matters {
        let url = resolve_repo(code, repo)?;
        copied += stage_one(code, &url, git_ref, keep, &workspace_root)?;
    }
    print!(
        "{}",
        staging_instructions(copied, matters.len(), &staged_root(&workspace_root))
    );
    Ok(())
}

/// Clone, build, and stage every sample matter's application for `root`,
/// each from the repository the compiled-in fixture records for it.
///
/// The development orchestrator calls this before it renders `.devx/env`, so
/// the following `web` process always reads freshly staged bundles. It reads
/// the compiled-in URLs rather than the store because it runs before the first
/// boot has seeded anything.
///
/// **Best-effort, per matter.** One application that will not clone or build
/// does not fail the boot: it is reported and skipped, and the seed publishes
/// that matter's deterministic portal document instead. Bringing up the whole
/// local tier is not the moment to discover that somebody else's sample
/// repository is having a bad day, and the two other matters have nothing to do
/// with it. `dev sample-project` is the strict path — an operator who asked for
/// a refresh wants the error, not a warning they have to go looking for.
pub(super) fn run_for_root(keep: bool, workspace_root: &Path) -> Result<usize> {
    super::require_tools(&["git", "pnpm"])?;
    let mut copied = 0;
    let mut skipped = Vec::new();
    for code in store::seed::sample_matter_codes() {
        let Some(url) = store::seed::sample_matter_repository(code) else {
            skipped.push(code);
            continue;
        };
        match stage_one(code, url, None, keep, workspace_root) {
            Ok(count) => copied += count,
            Err(error) => {
                eprintln!("navigator: could not stage `{code}` from {url}: {error:#}");
                skipped.push(code);
            }
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "navigator: {} matter(s) will serve the built-in portal document instead of a built \
             bundle: {}. Run `navigator dev sample-project --project <code>` to see the failure \
             on its own.",
            skipped.len(),
            skipped.join(", ")
        );
    }
    Ok(copied)
}

/// Clone, build, and stage one matter's application, returning how many files
/// landed in its staging directory.
fn stage_one(
    project_code: &str,
    repo: &str,
    git_ref: Option<&str>,
    keep: bool,
    workspace_root: &Path,
) -> Result<usize> {
    // The checkout and the build live in a temp tree; only `dist/` survives.
    let temp = tempfile::Builder::new()
        .prefix("navigator-sample-project-")
        .tempdir()
        .context("creating a temporary build directory")?;
    let checkout = temp.path().join("checkout");

    println!("navigator: cloning {repo}");
    let args = clone_args(repo, git_ref, &checkout);
    run_in(temp.path(), "git", &args)?;

    let (manifest, code) = declared_project(&checkout)?;
    println!(
        "navigator: {} declares Project `{code}`",
        repo_basename(repo)
    );

    require_lockfile(&checkout, repo)?;

    println!("navigator: installing dependencies (pnpm)");
    run_in(
        &checkout,
        "pnpm",
        &["install".to_string(), "--frozen-lockfile".to_string()],
    )?;

    println!("navigator: building the bundle (pnpm build)");
    run_in(&checkout, "pnpm", &["build".to_string()])?;

    let built = built_bundle(&checkout)?;

    // Refuse a bundle that declares a different matter, here rather than at
    // boot. The staging directory is named for the matter it belongs to, so
    // staging one matter's bundle under another's code would be the one
    // mistake that puts a client's application on another client's portal.
    // Boot checks this again against the manifest it re-reads; this is the
    // earlier, clearer failure.
    if code != project_code {
        bail!(
            "{repo} declares Project `{code}`, but it is being staged for `{project_code}`. \
             One matter's application must not mount on another's portal."
        );
    }

    // Stage the manifest beside the bundle: boot re-reads the declared Project
    // rather than trusting the directory it was found in.
    let stage = staged_for(workspace_root, project_code);
    let copied = copy_tree(&built, &stage.join(store::sample_project::DIST_DIR))?;
    std::fs::write(stage.join(store::sample_project::MANIFEST_FILE), &manifest)
        .with_context(|| format!("staging the manifest in {}", stage.display()))?;

    if keep {
        // Leak the TempDir so the tree survives for inspection.
        let path = temp.keep();
        println!("navigator: kept the build tree at {}", path.display());
    }

    Ok(copied)
}

/// What to tell the operator once the bundles are staged.
///
/// Built as a string so the refresh output is covered by a focused test.
fn staging_instructions(copied: usize, matters: usize, stage: &Path) -> String {
    format!(
        "\nnavigator: staged {copied} file(s) across {matters} matter(s) under {}\n\n\
         The next `web` boot reads them from the generated `.devx/env`.\n",
        stage.display(),
    )
}

/// The repository's last path segment, for a readable progress line.
fn repo_basename(repo: &str) -> &str {
    repo.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(repo)
        .trim_end_matches(".git")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--repo` wins outright, and does not consult the store at all.
    ///
    /// The lookup panics if called: a flag that still needed a database would
    /// make the command unusable on a cold checkout, which is the one case the
    /// flag exists for.
    #[test]
    fn an_explicit_repo_wins_without_reading_the_store() {
        let chosen = choose_repo(
            "sample-litigation",
            Some("https://example.test/a-fork/x.git"),
            Some("https://example.test/compiled-in.git"),
            || panic!("the store must not be consulted when --repo is given"),
        )
        .expect("a repo");
        assert_eq!(chosen, "https://example.test/a-fork/x.git");
    }

    /// With no flag, the Project's own recorded URL is what gets cloned —
    /// whatever forge it names, and in preference to the compiled-in default.
    /// That preference is what keeps repointing a matter a data change.
    #[test]
    fn the_projects_recorded_url_beats_the_compiled_in_default() {
        let chosen = choose_repo(
            "sample-litigation",
            None,
            Some("https://github.test/neon/compiled-in.git"),
            || {
                Ok(Lookup::Project(Some(
                    "https://gitlab.example/a-group/a-project.git".to_string(),
                )))
            },
        )
        .expect("a repo");
        assert_eq!(chosen, "https://gitlab.example/a-group/a-project.git");
    }

    /// A store nobody has seeded yet falls back to the matter's compiled-in
    /// repository, because that is the URL the seed is about to write. This is
    /// the first-boot path: the orchestrator stages the bundles *before*
    /// anything has created a Project row to read.
    #[test]
    fn an_unseeded_store_falls_back_to_the_matters_own_repository() {
        let chosen = choose_repo(
            "sample-transactional",
            None,
            Some("https://github.test/neon/navigator-sample-project-transactional"),
            || Ok(Lookup::NoProject),
        )
        .expect("a repo");
        assert_eq!(
            chosen,
            "https://github.test/neon/navigator-sample-project-transactional"
        );
    }

    /// The two absences are different problems, so they get different
    /// messages — and a row that exists with an empty URL is still an error.
    /// Something deliberately cleared it, so falling back would overwrite that
    /// decision with a default the operator did not ask for.
    #[test]
    fn each_absence_names_its_own_fix() {
        let no_project = choose_repo("sample-litigation", None, None, || Ok(Lookup::NoProject))
            .expect_err("a store with no such Project and no default is an error");
        let message = no_project.to_string();
        assert!(
            message.contains("start `web` once") && message.contains("--repo"),
            "the no-Project error must name both fixes: {message}"
        );

        let no_url = choose_repo(
            "sample-litigation",
            None,
            Some("https://github.test/neon/compiled-in.git"),
            || Ok(Lookup::Project(None)),
        )
        .expect_err("a Project with no repository URL is an error");
        let message = no_url.to_string();
        assert!(
            message.contains("records no repository URL"),
            "the no-URL error must say the column is empty: {message}"
        );
        assert!(
            !message.contains("compiled-in"),
            "a cleared URL must not be silently refilled from the default: {message}"
        );
    }

    /// A failed lookup propagates rather than being read as "no URL recorded".
    ///
    /// Otherwise an unreachable database would produce the *set one on the
    /// matter* advice, sending the reader to fix a row that is probably fine —
    /// or worse, silently build the compiled-in default.
    #[test]
    fn a_failed_lookup_propagates_instead_of_becoming_an_absence() {
        let error = choose_repo("sample-litigation", None, Some("https://x/y.git"), || {
            anyhow::bail!("connection refused")
        })
        .expect_err("a lookup failure is an error");
        assert!(
            error.to_string().contains("connection refused"),
            "the underlying failure must survive: {error}"
        );
    }

    /// No `--project` refreshes everything; naming one narrows to it; a name
    /// that is not a matter is refused with the list rather than refreshing
    /// nothing and reporting success.
    #[test]
    fn choosing_matters_defaults_to_all_and_refuses_an_unknown_name() {
        let known = ["sample-litigation", "sample-transactional", "sample-estate"];

        assert_eq!(choose_matters(None, &known).expect("all"), known.to_vec());
        assert_eq!(
            choose_matters(Some("sample-transactional"), &known).expect("one"),
            vec!["sample-transactional"]
        );

        let error = choose_matters(Some("no-such-matter"), &known)
            .expect_err("a name that is not a matter must fail");
        let message = error.to_string();
        assert!(
            message.contains("not a sample matter") && message.contains("sample-litigation"),
            "the error must list the real matters: {message}"
        );
    }

    /// Every matter the seed carries resolves a compiled-in repository, so the
    /// orchestrator's first-boot staging can never be handed a `None`.
    #[test]
    fn every_sample_matter_records_a_repository() {
        let codes = store::seed::sample_matter_codes();
        assert!(!codes.is_empty(), "the fixture carries at least one matter");
        for code in codes {
            let url = store::seed::sample_matter_repository(code)
                .unwrap_or_else(|| panic!("`{code}` records a repository"));
            assert!(url.starts_with("https://"), "`{code}` -> {url}");
        }
        assert_eq!(store::seed::sample_matter_repository("nope"), None);
    }

    /// A missing or unusable manifest is refused before a build is spent, and
    /// the text is returned verbatim for staging.
    #[test]
    fn the_declared_project_is_read_before_a_build_is_spent() {
        let dir = tempfile::tempdir().expect("tempdir");

        let missing =
            declared_project(dir.path()).expect_err("a checkout with no manifest is refused");
        assert!(
            missing
                .to_string()
                .contains(store::sample_project::MANIFEST_FILE),
            "the refusal must name the file a bundle declares its Project in: {missing}"
        );

        // Present but naming something that is not a Project code.
        std::fs::write(
            dir.path().join(store::sample_project::MANIFEST_FILE),
            b"project: \"../etc\"\n",
        )
        .expect("write");
        declared_project(dir.path()).expect_err("a manifest cannot smuggle a path segment");

        std::fs::write(
            dir.path().join(store::sample_project::MANIFEST_FILE),
            b"project: sample-litigation\n",
        )
        .expect("write");
        let (manifest, code) = declared_project(dir.path()).expect("a valid manifest");
        assert_eq!(code, "sample-litigation");
        assert_eq!(
            manifest, "project: sample-litigation\n",
            "the text is staged verbatim, so it must come back unaltered"
        );
    }

    /// A checkout with no lockfile is refused before an install is spent on it,
    /// and the message names the repository so a contributor knows where to
    /// commit one.
    #[test]
    fn a_checkout_without_a_lockfile_is_refused_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = require_lockfile(dir.path(), "https://forge.example/o/r")
            .expect_err("a checkout with no lockfile must be refused");
        let message = error.to_string();
        assert!(
            message.contains("https://forge.example/o/r") && message.contains("pnpm-lock.yaml"),
            "the refusal must name the repository and the file: {message}"
        );

        std::fs::write(dir.path().join("pnpm-lock.yaml"), b"lockfileVersion: '9.0'")
            .expect("write");
        require_lockfile(dir.path(), "https://forge.example/o/r").expect("a lockfile is enough");
    }

    /// The two failed-build shapes are reported separately, because they have
    /// different causes and different fixes.
    #[test]
    fn a_failed_build_names_which_half_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");

        let no_dist = built_bundle(dir.path()).expect_err("no dist/ is a failed build");
        assert!(
            no_dist.to_string().contains("no `dist/`"),
            "{no_dist}: a missing dist must point at the build script"
        );

        // A `dist/` full of assets but with no entry document: the build ran and
        // produced files nothing can point at.
        let dist = dir.path().join(store::sample_project::DIST_DIR);
        std::fs::create_dir_all(dist.join("assets")).expect("mkdir");
        std::fs::write(dist.join("assets/app-abc123.js"), b"x").expect("write");
        let no_entry = built_bundle(dir.path()).expect_err("no index.html is a failed build");
        assert!(
            no_entry
                .to_string()
                .contains(store::sample_project::ENTRY_DOCUMENT),
            "{no_entry}: a missing entry document must be named"
        );

        std::fs::write(
            dist.join(store::sample_project::ENTRY_DOCUMENT),
            b"<!doctype html>",
        )
        .expect("write");
        assert_eq!(
            built_bundle(dir.path()).expect("a complete build"),
            dist,
            "a dist/ with an entry document is the bundle to stage"
        );
    }

    /// The refresh output names the staged path and the generated environment.
    #[test]
    fn the_instructions_name_the_key_boot_reads_and_the_staged_path() {
        let text = staging_instructions(5, 3, Path::new("/w/.devx/sample-projects"));
        assert!(
            text.contains("The next `web` boot reads them from the generated `.devx/env`."),
            "{text}"
        );
        assert!(
            text.contains("staged 5 file(s) across 3 matter(s)"),
            "{text}"
        );
        assert!(text.contains("/w/.devx/sample-projects"), "{text}");
    }

    /// `run_in` reports a failing command and a missing one differently.
    ///
    /// These are the two failures an operator hits — a `pnpm build` that exits
    /// nonzero, and a `pnpm` that is not installed — and they need different
    /// fixes, so the missing-program case carries the "is it installed" hint
    /// rather than an exit status.
    #[test]
    fn a_failing_command_and_a_missing_one_are_reported_differently() {
        let dir = tempfile::tempdir().expect("tempdir");

        run_in(dir.path(), "true", &[]).expect("a succeeding command is Ok");

        let failed = run_in(dir.path(), "false", &[]).expect_err("a nonzero exit must fail");
        assert!(
            failed.to_string().contains("failed in"),
            "a nonzero exit must name where it ran: {failed}"
        );

        let missing = run_in(dir.path(), "navigator-no-such-program", &[])
            .expect_err("a missing program must fail");
        assert!(
            missing.to_string().contains("is it installed and on PATH?"),
            "a missing program must say so rather than report an exit code: {missing}"
        );
    }

    #[test]
    fn clone_is_shallow_and_single_branch() {
        let args = clone_args("https://example.com/r.git", None, Path::new("/tmp/x"));
        assert_eq!(
            args,
            vec![
                "clone",
                "--depth",
                "1",
                "--single-branch",
                "https://example.com/r.git",
                "/tmp/x"
            ]
        );
    }

    #[test]
    fn a_pinned_ref_becomes_branch_and_stays_shallow() {
        let args = clone_args("r.git", Some("v1.2.3"), Path::new("/tmp/x"));
        assert!(args.contains(&"--branch".to_string()));
        assert!(args.contains(&"v1.2.3".to_string()));
        assert_eq!(
            args.iter().filter(|a| *a == "--depth").count(),
            1,
            "pinning a ref must not cost a full history"
        );
    }

    /// One staging root, one subdirectory per matter. `web` reads the root
    /// from a single generated variable and finds every bundle under it.
    #[test]
    fn staged_paths_live_under_devx_one_directory_per_matter() {
        assert_eq!(
            staged_root(Path::new("/w")),
            PathBuf::from("/w/.devx/sample-projects")
        );
        assert_eq!(
            staged_for(Path::new("/w"), "sample-estate"),
            PathBuf::from("/w/.devx/sample-projects/sample-estate")
        );
    }

    #[test]
    fn repo_basename_reads_through_the_git_suffix_and_trailing_slash() {
        assert_eq!(
            repo_basename("https://github.com/o/navigator-sample-project-estate.git"),
            "navigator-sample-project-estate"
        );
        // SCP-style remotes split on the same `/` as a URL path.
        assert_eq!(repo_basename("git@github.com:o/r.git"), "r");
        assert_eq!(repo_basename("https://x/y/"), "y");
    }

    #[test]
    fn copy_tree_replaces_rather_than_merges() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(src.join("assets")).expect("mkdir");
        std::fs::write(src.join("index.html"), b"new").expect("write");
        std::fs::write(src.join("assets/app-new.js"), b"new").expect("write");

        // A previous build left an asset that the new one does not have.
        std::fs::create_dir_all(dst.join("assets")).expect("mkdir");
        std::fs::write(dst.join("assets/app-old.js"), b"old").expect("write");

        let copied = copy_tree(&src, &dst).expect("copy");

        assert_eq!(copied, 2);
        assert!(dst.join("assets/app-new.js").is_file());
        assert!(
            !dst.join("assets/app-old.js").exists(),
            "a stale asset would be republished on every boot"
        );
    }

    #[test]
    fn copy_tree_preserves_nested_structure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(src.join("assets/fonts")).expect("mkdir");
        std::fs::write(src.join("index.html"), b"x").expect("write");
        std::fs::write(src.join("assets/fonts/gorp.woff2"), b"x").expect("write");

        copy_tree(&src, &dst).expect("copy");

        assert!(dst.join("assets/fonts/gorp.woff2").is_file());
    }
}
