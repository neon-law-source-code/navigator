//! `navigator ops cut-release` — name today's UTC `YY.M.D` and write it as
//! the workspace version, or fail if that name is not a new release.
//!
//! This is the programmatic cut: [`crate::release_default_tag`] answers
//! "what would today be called?", and [`crate::release_version`] writes a
//! name it is given. Those stay separate because `ops release version` still
//! derives nothing — a clock-derived `--tag` is how `deploy.yml` once
//! published names the source never wrote. This command is allowed to look
//! at the clock because naming today *is* its job.
//!
//! [`crate::release_default_tag`] exits 0 when today is already covered —
//! that is the ordinary probe. This command is the opposite: an operator
//! (or a cron) that asked to cut, and cannot, must not read that as
//! success.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::release_check::{fetch_tags, release_tags};

/// What today's UTC date means against the published tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Today's `YY.M.D` is newer than every published release.
    Cut(String),
    /// A version at or past today is already published.
    AlreadyCovered {
        today: String,
        published: Option<String>,
    },
}

/// Decide whether today's UTC date names a release worth writing.
#[must_use]
pub(crate) fn decide(now: DateTime<Utc>, tags: &[String]) -> Decision {
    let today = crate::release::today_tag(now);
    match crate::release::default_tag(now, tags) {
        Some(version) => Decision::Cut(version.to_string()),
        None => Decision::AlreadyCovered {
            today,
            published: crate::release::highest_release(tags).map(|version| version.to_string()),
        },
    }
}

/// Entry point for `ops cut-release`.
pub fn run(
    now: DateTime<Utc>,
    repo: &Path,
    fetch: bool,
    manifest_path: &Path,
    no_commit: bool,
    dry_run: bool,
) -> ExitCode {
    match decide_from_repo(now, repo, fetch) {
        Ok(Decision::Cut(tag)) if dry_run => {
            println!("{tag}");
            eprintln!("navigator: cut-release: --dry-run: would write {tag} and commit it");
            ExitCode::SUCCESS
        }
        Ok(Decision::Cut(tag)) => {
            println!("navigator: cut-release: today's UTC date names {tag}");
            crate::release_version::run(manifest_path, &tag, no_commit)
        }
        Ok(Decision::AlreadyCovered { today, published }) => {
            match published {
                Some(published) => eprintln!(
                    "navigator: cut-release: today's UTC date names {today}, which is not a new \
                     release — {published} is already published"
                ),
                None => eprintln!(
                    "navigator: cut-release: today's UTC date names {today}, which is not a new \
                     release"
                ),
            }
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("navigator: cut-release: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn decide_from_repo(now: DateTime<Utc>, repo: &Path, fetch: bool) -> Result<Decision> {
    if fetch {
        fetch_tags(repo).context("fetch the release tags")?;
    }
    let tags = release_tags(repo).context("list the release tags")?;
    Ok(decide(now, &tags))
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

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn today_is_cuttable_when_nothing_is_published() {
        assert_eq!(
            decide(utc(2026, 9, 24), &[]),
            Decision::Cut("26.9.24".to_string())
        );
    }

    #[test]
    fn today_is_cuttable_when_it_is_newer_than_every_release() {
        assert_eq!(
            decide(utc(2026, 9, 24), &tags(&["26.9.23", "26.9.23-rc.1"])),
            Decision::Cut("26.9.24".to_string())
        );
    }

    /// A published prerelease of today does not consume the ordinary name:
    /// semver ranks `26.9.24-rc.1` below `26.9.24`.
    #[test]
    fn a_prerelease_of_today_does_not_block_the_ordinary_cut() {
        assert_eq!(
            decide(utc(2026, 9, 24), &tags(&["26.9.24-rc.1"])),
            Decision::Cut("26.9.24".to_string())
        );
    }

    #[test]
    fn fails_when_today_is_already_published() {
        assert_eq!(
            decide(utc(2026, 9, 24), &tags(&["26.9.24"])),
            Decision::AlreadyCovered {
                today: "26.9.24".to_string(),
                published: Some("26.9.24".to_string()),
            }
        );
    }

    #[test]
    fn fails_when_a_later_version_is_already_published() {
        assert_eq!(
            decide(utc(2026, 9, 24), &tags(&["26.9.25"])),
            Decision::AlreadyCovered {
                today: "26.9.24".to_string(),
                published: Some("26.9.25".to_string()),
            }
        );
    }

    #[test]
    fn names_the_highest_published_version_in_the_covered_decision() {
        assert_eq!(
            decide(utc(2026, 9, 24), &tags(&["26.9.23", "26.9.24"])),
            Decision::AlreadyCovered {
                today: "26.9.24".to_string(),
                published: Some("26.9.24".to_string()),
            }
        );
    }
}
