//! `navigator ops release check` — decide whether the version in
//! `[workspace.package].version` is a release that has not happened yet.
//!
//! This is the whole release trigger. A release is cut by bumping the workspace
//! version in an ordinary pull request and merging it; `deploy.yml` runs this
//! command on every push to `main`, and a `publishable=true` answer is what
//! makes that push build, prove, tag, and publish. Nothing derives a version
//! from a clock, and nobody pushes a tag by hand.
//!
//! # Why "is it already tagged?" is the right question
//!
//! The obvious implementation compares the manifest at `HEAD` against the
//! manifest at `HEAD^` and publishes when they differ. Comparing against the
//! TAGS instead answers the same question and three more for free: a re-run of a
//! run that already published is recognised as such, a revert that restores an
//! older version is caught as a regression rather than republished, and a
//! release whose run failed before it tagged can simply be re-run. The tags are
//! the immutable record of what shipped; `HEAD^` is a guess about what shipped.
//!
//! # Three answers, two exit codes
//!
//! Releasable and already-released are both ordinary and both exit 0 — most
//! commits on `main` carry no bump, and a gate that failed on them would fail
//! almost every push. A REGRESSION exits nonzero, because a manifest naming a
//! version older than one already published is a defect wherever it is found:
//! on a pull request it is a bad bump or a rebase that resurrected an old
//! manifest, and on `main` it is that same defect already landed.
//!
//! That is why `ci.yml` runs this on every pull request. It costs a tag listing
//! and it is the only check that sees a bad bump while the bump is still free to
//! change.

use std::path::Path;
use std::process::{Command, ExitCode, Output};

use anyhow::{bail, Context, Result};
use semver::Version;

use crate::release::{self, Standing};

/// What the check concluded, for the caller that has to report it.
#[derive(Debug)]
struct Outcome {
    version: Version,
    standing: Standing,
}

impl Outcome {
    fn publishable(&self) -> bool {
        matches!(self.standing, Standing::Releasable)
    }

    /// Whether this version is a semver prerelease.
    ///
    /// The GitHub Release is the one surface that treats a prerelease
    /// differently — it is flagged so GitHub stops reporting it as "Latest".
    /// The images and the CLI archives follow every publishable version,
    /// because a consumer resolving one version needs it to be the newest good
    /// build. The Homebrew tap is the one narrower surface — see
    /// [`Self::tap_follows`], which asks a different question of the same
    /// prerelease.
    fn prerelease(&self) -> bool {
        !self.version.pre.is_empty()
    }

    /// Whether the Homebrew tap will follow this version.
    ///
    /// The tap holds exactly one formula version and its own workflows accept
    /// only `YY.M.D` and `YY.M.D-hotfix.N`, so a release candidate is a tag it
    /// refuses outright rather than one it merely deprioritises. `deploy.yml`
    /// reads this to decide whether to dispatch at all: without it the deploy
    /// held a release-candidate policy its own tap did not, told the tap about
    /// `26.8.30-rc.1`, and then failed the release over a bump that was never
    /// going to happen.
    ///
    /// It is deliberately NOT `publishable && …`. The two outputs answer
    /// different questions — "did this commit cut a release?" and "is this a
    /// release the tap can carry?" — and folding them together would leave
    /// `deploy.yml` unable to say which one a skipped tap job answered.
    /// [`release::tap_follows`] holds the rule so nothing transcribes it here.
    fn tap_follows(&self) -> bool {
        release::tap_follows(&self.version)
    }
}

/// Entry point for `ops release check`.
pub fn run(manifest_path: &Path, repo: &Path, fetch: bool, github_output: bool) -> ExitCode {
    let outcome = match check(manifest_path, repo, fetch) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("navigator: release-check: {error:#}");
            return ExitCode::from(2);
        }
    };

    match &outcome.standing {
        Standing::Releasable => println!(
            "navigator: {} is a release: no tag names it yet, and it is newer than every version \
             already published",
            outcome.version
        ),
        Standing::AlreadyReleased => println!(
            "navigator: {} is already released — nothing to publish. Bump \
             `[workspace.package].version` to cut the next one",
            outcome.version
        ),
        Standing::Regression { highest } => {
            eprintln!(
                "navigator: release-check: the workspace version is {}, which is OLDER than the \
                 released {highest}. A release must be newer than every version already \
                 published: bump `[workspace.package].version` past {highest}, or restore the \
                 version this branch was meant to carry",
                outcome.version
            );
            return ExitCode::from(2);
        }
    }

    if github_output {
        if let Err(error) = write_github_output(&outcome) {
            eprintln!("navigator: release-check: {error:#}");
            return ExitCode::from(2);
        }
    }

    ExitCode::SUCCESS
}

fn check(manifest_path: &Path, repo: &Path, fetch: bool) -> Result<Outcome> {
    let manifest = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let declared = release::workspace_version(&manifest)
        .with_context(|| format!("read the release version from {}", manifest_path.display()))?;
    let version = release::parse(&declared).with_context(|| {
        format!(
            "the version in {} is not one a release could carry",
            manifest_path.display()
        )
    })?;

    // Without the tags there is no anchor, and a listing that silently came back
    // empty would report every version as releasable. Fetch by default: a
    // shallow `actions/checkout` carries no tags at all, and an operator's local
    // clone is usually a few releases behind.
    if fetch {
        fetch_tags(repo)?;
    }
    let tags = release_tags(repo)?;
    let highest = release::highest_release(&tags);
    let already_at_head = tag_points_at_head(repo, &version.to_string())?;

    Ok(Outcome {
        standing: release::standing(&version, highest.as_ref(), already_at_head),
        version,
    })
}

/// Whether the tag naming this version already exists AND names this very
/// commit.
///
/// This is what makes re-running a release idempotent. A release that failed
/// after the tag job has a ref, so the plain comparison would answer
/// "already released" and skip every publishing job — reporting success for
/// having published nothing. A tag on some ANCESTOR is a different thing
/// entirely, and the ordinary state of `main` after any release.
fn tag_points_at_head(repo: &Path, version: &str) -> Result<bool> {
    let head = rev_parse(repo, "HEAD")?;
    let Some(head) = head else {
        // A repository with no commits has no HEAD to match.
        return Ok(false);
    };
    // `^{commit}` peels an annotated tag and leaves a lightweight one alone, so
    // both spellings compare against the same thing.
    let tagged = rev_parse(repo, &format!("refs/tags/{version}^{{commit}}"))?;
    Ok(tagged.is_some_and(|tagged| tagged == head))
}

/// Resolve a revision, or `None` when it does not exist.
fn rev_parse(repo: &Path, revision: &str) -> Result<Option<String>> {
    let output = git(repo, &["rev-parse", "--verify", "--quiet", revision])
        .with_context(|| format!("resolve `{revision}`"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let resolved = String::from_utf8(output.stdout)
        .with_context(|| format!("`{revision}` did not resolve to UTF-8"))?
        .trim()
        .to_string();
    Ok((!resolved.is_empty()).then_some(resolved))
}

/// Refresh the release tags from `origin`.
///
/// `pub(crate)` because [`crate::release_default_tag`] fetches the same tags to
/// answer a different question — one listing, shared, rather than a second
/// `git fetch` copied alongside it.
pub(crate) fn fetch_tags(repo: &Path) -> Result<()> {
    let output = git(repo, &["fetch", "--tags", "--quiet", "origin"])
        .context("fetch the release tags from origin")?;
    if !output.status.success() {
        bail!(
            "could not fetch the release tags from origin: {}. Pass `--no-fetch` to compare \
             against the tags already in this clone",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Every tag in the repository that could name a release.
///
/// The glob is the coarse filter the `release-tags` ruleset also uses;
/// [`release::parse`] is what actually decides, so this returning a non-version
/// is expected and harmless.
///
/// `pub(crate)` for the same reason as [`fetch_tags`].
pub(crate) fn release_tags(repo: &Path) -> Result<Vec<String>> {
    let output = git(repo, &["tag", "--list", release::RELEASE_TAG_GLOB])
        .context("list the release tags")?;
    if !output.status.success() {
        bail!(
            "could not list the release tags: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8(output.stdout)
        .context("the tag listing is not UTF-8")?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Hand the answer to the workflow step that asked.
///
/// `deploy.yml` reads all four: `tag` becomes the `RELEASE_TAG` build-arg and
/// the ref the tag job creates, `publishable` gates every job after
/// integration, `prerelease` flags the GitHub Release, and `tap_follows` gates
/// the Homebrew hand-off alone.
fn write_github_output(outcome: &Outcome) -> Result<()> {
    use std::io::Write;

    let path = std::env::var("GITHUB_OUTPUT")
        .context("--github-output needs $GITHUB_OUTPUT, which only a GitHub Actions step sets")?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .with_context(|| format!("open {path} for append"))?;

    writeln!(file, "tag={}", outcome.version).context("write the release tag output")?;
    writeln!(file, "publishable={}", outcome.publishable())
        .context("write the publishable output")?;
    writeln!(file, "prerelease={}", outcome.prerelease()).context("write the prerelease output")?;
    writeln!(file, "tap_follows={}", outcome.tap_follows())
        .context("write the tap_follows output")?;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git").arg("-C").arg(repo).args(args).output()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with the tags a release decision is made against.
    ///
    /// Real git rather than a stubbed listing: the tag glob is passed to git and
    /// the parsing happens on what git actually prints, so a fake would only
    /// prove the fake.
    fn repo_with_tags(tags: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "release-check@example.com"],
            vec!["config", "user.name", "release check"],
            vec!["commit", "--quiet", "--allow-empty", "-m", "root"],
        ] {
            let output = git(path, &args).expect("git runs");
            assert!(output.status.success(), "git {args:?} failed");
        }
        for tag in tags {
            let output = git(path, &["tag", tag]).expect("git runs");
            assert!(output.status.success(), "tagging {tag} failed");
        }
        // ADVANCE PAST THE TAGS. `main` after a release is a commit the release
        // tag is an ancestor of, never the tagged commit itself, and the
        // distinction decides an answer: a tag pointing at HEAD means this commit
        // IS that release and re-publishing it is idempotent. A fixture that
        // tagged HEAD would put every case on that path.
        let output = git(
            path,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "after the release",
            ],
        )
        .expect("git runs");
        assert!(output.status.success(), "advancing past the tags failed");
        dir
    }

    fn manifest_at(dir: &Path, version: &str) -> std::path::PathBuf {
        let path = dir.join("Cargo.toml");
        std::fs::write(
            &path,
            format!("[workspace.package]\nrust-version = \"1.97.0\"\nversion = \"{version}\"\n"),
        )
        .expect("write the manifest");
        path
    }

    /// A version no tag names, above everything released, is the release.
    #[test]
    fn a_bump_past_every_tag_is_releasable() {
        let repo = repo_with_tags(&["26.8.20", "26.8.21-hotfix.12", "26.8.22"]);
        let manifest = manifest_at(repo.path(), "26.8.23");

        let outcome = check(&manifest, repo.path(), false).expect("a decision");
        assert_eq!(outcome.standing, Standing::Releasable);
        assert!(outcome.publishable());
        assert!(!outcome.prerelease());
    }

    /// The ordinary state of `main`: the manifest names the release that already
    /// happened. This must not be an error — it is almost every commit.
    #[test]
    fn the_version_already_tagged_publishes_nothing() {
        let repo = repo_with_tags(&["26.8.20", "26.8.22"]);
        let manifest = manifest_at(repo.path(), "26.8.22");

        let outcome = check(&manifest, repo.path(), false).expect("a decision");
        assert_eq!(outcome.standing, Standing::AlreadyReleased);
        assert!(!outcome.publishable());
    }

    /// A manifest behind the tags is a defect, and the message has to name the
    /// version it is behind or the reader cannot act on it.
    #[test]
    fn a_manifest_behind_the_tags_is_a_regression() {
        let repo = repo_with_tags(&["26.8.20", "26.8.22"]);
        let manifest = manifest_at(repo.path(), "26.8.21");

        let outcome = check(&manifest, repo.path(), false).expect("a decision");
        assert_eq!(
            outcome.standing,
            Standing::Regression {
                highest: Version::parse("26.8.22").expect("valid")
            }
        );
        assert!(!outcome.publishable());
    }

    /// A hotfix carried on top of the release it fixes is BEHIND it, and the
    /// check says so instead of publishing a version consumers would resolve
    /// backwards. The next hotfix hangs off the next version — which nothing
    /// here has to know, because the comparator does.
    #[test]
    fn a_prerelease_of_an_already_released_version_is_refused() {
        let repo = repo_with_tags(&["26.8.22"]);
        let behind = manifest_at(repo.path(), "26.8.22-hotfix.1");
        assert!(matches!(
            check(&behind, repo.path(), false)
                .expect("a decision")
                .standing,
            Standing::Regression { .. }
        ));

        let ahead = manifest_at(repo.path(), "26.8.23-hotfix.1");
        let outcome = check(&ahead, repo.path(), false).expect("a decision");
        assert_eq!(outcome.standing, Standing::Releasable);
        assert!(
            outcome.prerelease(),
            "a `-hotfix.N` version must report itself as a prerelease so the GitHub Release is \
             flagged"
        );
    }

    /// A release candidate publishes everywhere except the tap.
    ///
    /// This is the whole of item 1: the deploy used to hold no shape filter, so
    /// `26.8.30-rc.1` was dispatched to a tap whose own guard refuses it. The
    /// answer is one more output, not a second copy of the tap's regex — the
    /// version has already been parsed by the time this is asked.
    #[test]
    fn a_release_candidate_publishes_but_the_tap_is_not_told() {
        let repo = repo_with_tags(&["26.8.28"]);
        let manifest = manifest_at(repo.path(), "26.8.30-rc.1");
        let outcome = check(&manifest, repo.path(), false).expect("a decision");

        assert_eq!(
            outcome.standing,
            Standing::Releasable,
            "an rc is still a release: it proves the workspace, publishes every image, and cuts the archives"
        );
        assert!(outcome.publishable());
        assert!(outcome.prerelease());
        assert!(
            !outcome.tap_follows(),
            "the tap refuses this shape, so a deploy that dispatches it fails the release over a bump that was never going to run"
        );
    }

    /// A hotfix is told to the tap, and that is the assertion the 404 was made
    /// of.
    ///
    /// The formula holds exactly one version, so excluding hotfixes strands
    /// `brew install` on the last ordinary release for as long as a run of
    /// hotfixes lasts. Narrowing the gate to release candidates must not
    /// re-introduce that.
    #[test]
    fn a_hotfix_is_still_told_to_the_tap() {
        let repo = repo_with_tags(&["26.8.22"]);
        let manifest = manifest_at(repo.path(), "26.8.23-hotfix.1");
        let outcome = check(&manifest, repo.path(), false).expect("a decision");

        assert!(outcome.publishable());
        assert!(outcome.prerelease(), "a hotfix is a semver prerelease");
        assert!(
            outcome.tap_follows(),
            "a hotfix is a shape the tap publishes — it did so for `26.8.26-hotfix.1`"
        );
    }

    /// An ordinary release is told to the tap.
    #[test]
    fn an_ordinary_release_is_told_to_the_tap() {
        let repo = repo_with_tags(&["26.8.22"]);
        let manifest = manifest_at(repo.path(), "26.8.23");
        let outcome = check(&manifest, repo.path(), false).expect("a decision");

        assert!(outcome.publishable());
        assert!(!outcome.prerelease());
        assert!(outcome.tap_follows());
    }

    /// A repository with no releases yet publishes whatever it is handed.
    #[test]
    fn a_repository_with_no_release_tags_can_cut_its_first() {
        let repo = repo_with_tags(&[]);
        let manifest = manifest_at(repo.path(), "0.1.0");

        assert_eq!(
            check(&manifest, repo.path(), false)
                .expect("a decision")
                .standing,
            Standing::Releasable
        );
    }

    /// Tags that are not releases cannot raise the bar a bump has to clear.
    #[test]
    fn non_release_tags_are_ignored() {
        let repo = repo_with_tags(&["latest", "26.8.20", "99-not-a-version"]);
        let manifest = manifest_at(repo.path(), "26.8.21");

        assert_eq!(
            check(&manifest, repo.path(), false)
                .expect("a decision")
                .standing,
            Standing::Releasable
        );
    }

    /// A release whose run died after tagging must still publish when it is
    /// re-run. The tag names this very commit, so republishing is idempotent —
    /// and the alternative is a full re-run that skips every publishing job and
    /// reports success for having done nothing.
    #[test]
    fn a_release_whose_tag_already_names_this_commit_is_still_publishable() {
        let repo = repo_with_tags(&["26.8.20"]);
        let manifest = manifest_at(repo.path(), "26.8.23");

        // Before the tag job: an ordinary releasable commit.
        let outcome = check(&manifest, repo.path(), false).expect("a decision");
        assert_eq!(outcome.standing, Standing::Releasable);

        // The tag job runs, then the run is re-run from the top.
        let tagged = git(repo.path(), &["tag", "26.8.23"]).expect("git runs");
        assert!(tagged.status.success());

        let outcome = check(&manifest, repo.path(), false).expect("a decision");
        assert_eq!(
            outcome.standing,
            Standing::Releasable,
            "the tag names this commit, so the re-run must republish rather than skip"
        );
        assert!(outcome.publishable());
    }

    /// A manifest version a release could not carry fails the check rather than
    /// reaching the tag comparison — a version that cannot be parsed cannot be
    /// ordered, so there is nothing to compare.
    #[test]
    fn a_manifest_version_that_is_not_semver_fails_the_check() {
        let repo = repo_with_tags(&["26.8.20"]);
        let manifest = manifest_at(repo.path(), "26.08.21");

        let error = check(&manifest, repo.path(), false).expect_err("a refusal");
        assert!(
            format!("{error:#}").contains("26.08.21"),
            "the refusal must name the version it refused: {error:#}"
        );
    }
}
