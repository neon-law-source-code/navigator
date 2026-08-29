//! `navigator ops release-default-tag` — the version the `cut-release` skill
//! should hand to `ops release version --tag` when the operator names none:
//! today's UTC date under the `YY.M.D` convention, unless a release already
//! exists that makes today's date no improvement.
//!
//! `ops release version` itself still derives nothing and still requires
//! `--tag` — see its module doc for why, and `docs/gitops.md` for the
//! `deploy.yml` incident that made it a rule. This command does not change
//! that: it answers a narrower, upstream question — "what would today's date
//! even be called, and is it worth asking for?" — so an operator (or a skill
//! acting for one) can still be the one who types `--tag`. Nothing in
//! `deploy.yml` calls this, and it writes nothing.
//!
//! Prints the bare tag on stdout and nothing else when today is releasable,
//! so a caller can capture it directly: `tag=$(navigator ops
//! release-default-tag)`. Prints nothing to stdout — only a human-readable
//! reason on stderr — when today is already covered, so an empty capture
//! means "nothing to cut" rather than a value to go parse.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::release_check::{fetch_tags, release_tags};

/// Entry point for `ops release-default-tag`.
pub fn run(now: DateTime<Utc>, repo: &Path, fetch: bool) -> ExitCode {
    match suggest(now, repo, fetch) {
        Ok(Some(tag)) => {
            println!("{tag}");
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!(
                "navigator: release-default-tag: today's date names nothing new to release — a \
                 version at or past it is already published; nothing to do"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("navigator: release-default-tag: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn suggest(now: DateTime<Utc>, repo: &Path, fetch: bool) -> Result<Option<String>> {
    if fetch {
        fetch_tags(repo).context("fetch the release tags")?;
    }
    let tags = release_tags(repo).context("list the release tags")?;
    Ok(crate::release::default_tag(now, &tags).map(|version| version.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0)
            .single()
            .expect("a valid calendar date")
    }

    /// A real git repository, the way `release_check`'s own tests build one:
    /// a fake listing would only prove the fake, since the tag glob and the
    /// listing both run through git.
    fn repo_with_tags(tags: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "release-default-tag@example.com"],
            vec!["config", "user.name", "release default tag"],
            vec!["commit", "--quiet", "--allow-empty", "-m", "root"],
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(&args)
                .output()
                .expect("git runs");
            assert!(output.status.success(), "git {args:?} failed");
        }
        for tag in tags {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["tag", tag])
                .output()
                .expect("git runs");
            assert!(output.status.success(), "tagging {tag} failed");
        }
        dir
    }

    /// A fresh repository with no releases yet suggests today, unfetched —
    /// `--no-fetch` must not stop the local tag listing from being read.
    #[test]
    fn suggests_todays_date_with_no_releases_yet() {
        let repo = repo_with_tags(&[]);
        assert_eq!(
            suggest(utc(2026, 8, 22), repo.path(), false).expect("a decision"),
            Some("26.8.22".to_string())
        );
    }

    /// Today's date is newer than everything released: it is suggested.
    #[test]
    fn suggests_todays_date_past_every_release() {
        let repo = repo_with_tags(&["26.8.20", "26.8.21-hotfix.4"]);
        assert_eq!(
            suggest(utc(2026, 8, 22), repo.path(), false).expect("a decision"),
            Some("26.8.22".to_string())
        );
    }

    /// A release already exists for today: nothing is suggested, and it is
    /// not an error — asking twice in one day is ordinary.
    #[test]
    fn suggests_nothing_when_today_is_already_released() {
        let repo = repo_with_tags(&["26.8.22"]);
        assert_eq!(
            suggest(utc(2026, 8, 22), repo.path(), false).expect("a decision"),
            None
        );
    }

    /// A LATER version is already released than today's date would name.
    /// Still not an error: there is nothing today's date would publish that
    /// is not already superseded.
    #[test]
    fn suggests_nothing_when_a_later_version_is_already_released() {
        let repo = repo_with_tags(&["26.8.23"]);
        assert_eq!(
            suggest(utc(2026, 8, 22), repo.path(), false).expect("a decision"),
            None
        );
    }

    /// A repository that fails to fetch (no `origin`, no network) surfaces
    /// the git error rather than silently comparing against a stale or empty
    /// tag list.
    #[test]
    fn a_fetch_failure_is_reported_rather_than_swallowed() {
        let repo = repo_with_tags(&[]);
        let error = suggest(utc(2026, 8, 22), repo.path(), true).expect_err("no origin to fetch");
        assert!(format!("{error:#}").contains("fetch"));
    }
}
