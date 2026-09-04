//! The one place Navigator answers two questions about a release version: is
//! this a version at all, and is it newer than the last one published?
//!
//! Both answers come from the `semver` crate — the same implementation Cargo
//! resolves with — rather than from a grammar written here. That is the whole
//! point of this module. The rules used to exist in four hand-transcribed
//! copies: a `grep -E` in `deploy.yml`, a bash regex in the `cut-release`
//! scripts, ninety lines of component checking in `release_version.rs`, and a
//! `parse_release_tag` in `devx::registry` with its own ordering. Four copies of
//! a rule are four chances for them to disagree, and they did: `registry`
//! ordered `26.8.22-hotfix.22` ABOVE `26.8.22`, where semver — and therefore
//! Cargo, and therefore every consumer that resolves a version — orders it
//! below.
//!
//! # The shape is semver, and nothing more
//!
//! `Version::parse` already refuses everything the hand-written grammar
//! refused: a fourth component, a missing component, a padded component
//! (`26.08.20` — leading zeros are invalid in both the numeric core and a
//! numeric prerelease identifier), and a malformed prerelease. It refuses them
//! against the spec rather than against our transcription of it.
//!
//! What it does NOT check is the calendar, and that is deliberate.
//! `YY.M.D[-hotfix.N]` is the firm's release CONVENTION, documented in the
//! `cut-release` skill and in `docs/gitops.md`, and an operator is free to name
//! a version that departs from it. A release is cut by bumping
//! `[workspace.package].version` and merging that to `main`, so the version is
//! written days before it publishes and a clock check here could only ever fail
//! a bump for having been reviewed too slowly.
//!
//! # Ordering is the invariant that replaced the calendar
//!
//! What the date check really bought was uniqueness: `YY.M.D` admits one
//! release per UTC day, so a second cut needed a `-hotfix.N` prerelease, which
//! in turn had to hang off TOMORROW's date because semver ranks a prerelease
//! below its own base. That rule was a hand-written description of what
//! `Version::cmp` computes. Comparing against the highest released version
//! directly makes it a consequence instead of a rule anyone has to know:
//!
//! ```text
//! 26.8.21  <  26.8.22-hotfix.3  <  26.8.22-hotfix.21  <  26.8.22
//! ```
//!
//! [`highest_release`] is therefore the anchor every caller compares against,
//! and it sorts with `semver`. It must never be replaced with `git tag --sort
//! -v:refname`: git's version sort is not a semver comparator and ranks
//! `26.8.22-hotfix.22` above `26.8.22`, which is the exact inversion this
//! module exists to have none of.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Utc};
use semver::Version;

/// The glob `deploy.yml` and the `release-tags` ruleset both use to name a
/// release ref.
///
/// Kept here so the one place that lists tags and the one place that parses
/// them cannot drift apart. It is deliberately looser than [`parse`] — fnmatch's
/// `*` matches dots, so this admits plenty of strings that are not versions —
/// because its job is to narrow a repository's tags to candidates, and [`parse`]
/// is what decides.
pub const RELEASE_TAG_GLOB: &str = "[0-9]*.[0-9]*.[0-9]*";

/// Parse a release version, or explain why it is not one.
///
/// This is semver's own parser plus exactly one additional rule: BUILD METADATA
/// IS REFUSED. Two reasons, and either alone is sufficient.
///
/// `+` IS NOT A LEGAL CHARACTER IN AN OCI IMAGE TAG. Every release version is
/// stamped onto four image tags, so a version carrying build metadata could not
/// name the images it names — the registry push would fail after the tag ref was
/// already created and irreversible.
///
/// AND ITS PRECEDENCE IS NOT PORTABLE. Spec §10 says build metadata MUST be
/// ignored when determining precedence; the `semver` crate compares it anyway,
/// so `26.8.22+a < 26.8.22+b` here and equal by the specification. Whichever is
/// right, a release ordering that depends on the answer is one where this
/// pipeline and a consumer resolving the same versions can legitimately
/// disagree — see the test, which pins the crate's actual behaviour rather than
/// the spec's.
pub fn parse(text: &str) -> Result<Version> {
    let text = text.trim();
    if text.is_empty() {
        bail!("a release version must not be empty");
    }

    let version = Version::parse(text).with_context(|| {
        format!(
            "`{text}` is not a release version: it must be semver — three dot-separated \
             components with no leading zeros, optionally suffixed with a prerelease such as \
             `-hotfix.3` (the firm's convention is `YY.M.D`, so August 22nd 2026 is `26.8.22`)"
        )
    })?;

    if !version.build.is_empty() {
        bail!(
            "`{text}` carries the build metadata `+{}`, which a release version may not: semver \
             ignores build metadata when ordering, so two releases differing only there would \
             compare equal, and `+` is not a legal character in an image tag",
            version.build
        );
    }

    Ok(version)
}

/// True when `text` is a release version.
///
/// The predicate half of [`parse`], for callers that only need to refuse a
/// non-release ref — a rolling `latest`, a `buildcache`, a `ci-<sha>` — rather
/// than to report why.
#[must_use]
pub fn is_release_version(text: &str) -> bool {
    parse(text).is_ok()
}

/// Read `[workspace.package].version` out of a workspace manifest.
///
/// The table is addressed rather than searched. `[workspace.dependencies]` holds
/// dozens of `version =` lines and `rust-version` sits in this very table, so a
/// file-wide search for a `version` key is a check that passes and proves
/// nothing — which is how a `deploy.yml` grep once compared a release tag
/// against the first dependency pin it found.
pub fn workspace_version(manifest_toml: &str) -> Result<String> {
    let document: toml::Value =
        toml::from_str(manifest_toml).context("parse the workspace manifest as TOML")?;

    let version = document
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the manifest has no `version` under [workspace.package] — the shape every crate \
                 inherits through `version.workspace = true` has moved"
            )
        })?;

    Ok(version.to_string())
}

/// The highest release version among `tags`, or `None` when none of them is one.
///
/// Unparseable tags are discarded rather than refused. A repository accumulates
/// refs that are not releases, and one legacy four-component tag (`26.8.19.4`,
/// the spelling that predates `-hotfix.N`) must not stop a release from being
/// cut — every such tag names a version older than anything current, so
/// dropping it cannot raise the bar this function computes.
#[must_use]
pub fn highest_release(tags: &[String]) -> Option<Version> {
    tags.iter().filter_map(|tag| parse(tag).ok()).max()
}

/// Whether `candidate` may be published given everything already released.
///
/// Three outcomes rather than a boolean, because the three mean different
/// things to a caller and collapsing them loses the one that matters. A version
/// EQUAL to the highest release is the ordinary state of `main` — most commits
/// carry no bump — and must not be an error. A version BELOW it is a real
/// defect: a bad bump, or a rebase that resurrected an old manifest, and it
/// would publish a version whose consumers already have something newer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Newer than every released version: this is a release.
    Releasable,
    /// Already released under this exact version. Nothing to publish.
    AlreadyReleased,
    /// Older than a version already released.
    Regression { highest: Version },
}

/// Compare a candidate version against the highest already-released one.
///
/// `already_at_head` is whether the tag naming `candidate` exists AND points at
/// the commit being checked. When it does, this commit IS that release, and
/// publishing it again is idempotent rather than duplicate — which is what makes
/// a re-run of a release that died after tagging work. Without it, "re-run all
/// jobs" would re-ask this question, get `AlreadyReleased`, and skip every
/// publishing job while reporting success: the silent no-op that looks exactly
/// like a clean run.
///
/// A tag pointing at some ANCESTOR of the commit being checked is the ordinary
/// state of `main` after a release and is not this case.
#[must_use]
pub fn standing(
    candidate: &Version,
    released: Option<&Version>,
    already_at_head: bool,
) -> Standing {
    if already_at_head {
        return Standing::Releasable;
    }
    match released {
        None => Standing::Releasable,
        Some(highest) if candidate > highest => Standing::Releasable,
        Some(highest) if candidate == highest => Standing::AlreadyReleased,
        Some(highest) => Standing::Regression {
            highest: highest.clone(),
        },
    }
}

/// Format `now` as the `YY.M.D` release-naming convention: two-digit year,
/// unpadded month, unpadded day. `now` being August 22nd 2026 UTC gives
/// `26.8.22`.
///
/// This is a NAME, not a validity claim. It is `pub(crate)` because
/// [`crate::release_default_tag`] is the only caller with a clock to give it
/// — an operator names a release explicitly, through `ops release version
/// --tag`, and that command never reads one. See [`default_tag`] for where
/// this fits.
pub(crate) fn today_tag(now: DateTime<Utc>) -> String {
    format!("{}.{}.{}", now.year() % 100, now.month(), now.day())
}

/// The version `ops release-default-tag` should suggest when the operator
/// asks for one and names no version themselves: today's UTC date under the
/// `YY.M.D` convention, unless a release already exists that makes today's
/// date no improvement.
///
/// `None` is not an error. It is today's ordinary "nothing to cut" answer —
/// on a day the operator already released, or (had the clock skewed
/// backwards) on a day a later version already exists. Either way there is
/// nothing this candidate would publish that is not already published, which
/// is exactly [`Standing::AlreadyReleased`] and [`Standing::Regression`]. Only
/// [`Standing::Releasable`] is a name worth handing back.
///
/// This never reads a tag pointing at `HEAD`: unlike [`standing`]'s caller in
/// `release_check`, there is no commit yet whose manifest could carry this
/// candidate, so the idempotent-rerun case cannot arise here.
#[must_use]
pub(crate) fn default_tag(now: DateTime<Utc>, tags: &[String]) -> Option<Version> {
    let candidate = parse(&today_tag(now)).expect(
        "today_tag formats a plain three-component, unpadded, non-negative version, which parse \
         always accepts",
    );
    let highest = highest_release(tags);
    match standing(&candidate, highest.as_ref(), false) {
        Standing::Releasable => Some(candidate),
        Standing::AlreadyReleased | Standing::Regression { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The convention parses, and so does every shape an operator may reach for
    /// when the convention does not fit. None of these is a calendar assertion:
    /// the month, the day, and the prerelease label are the operator's to name.
    #[test]
    fn accepts_the_convention_and_anything_else_semver_accepts() {
        for text in [
            "26.8.22",
            "26.8.22-hotfix.22",
            "26.12.31",
            "26.8.22-hotfix.0",
            "26.8.22-hotfix.9999",
            "2026.8.22",
            "1.0.0",
            "0.1.0",
            "26.8.22-rc.1",
        ] {
            assert!(
                parse(text).is_ok(),
                "`{text}` must be an admissible release version"
            );
        }
    }

    /// Everything the ninety-line hand-written grammar refused, refused by
    /// `Version::parse` alone. This is the test that earns the deletion.
    #[test]
    fn refuses_everything_the_hand_written_grammar_refused() {
        for text in [
            "26.08.22",          // padded month
            "26.8.022",          // padded day
            "26.8.22.13",        // a fourth component
            "26.8",              // a missing component
            "26.8.22-hotfix.08", // padded prerelease number
            "v26.8.22",          // a `v` prefix
            "latest",            // a rolling tag
            "buildcache",        // a cache ref
            "ci-abc1234",        // a per-commit CI tag
            "26.8.22-",          // an empty prerelease
            "",
        ] {
            assert!(
                parse(text).is_err(),
                "`{text}` must not be an admissible release version"
            );
        }
    }

    /// A release may not carry build metadata: `+` cannot appear in an image tag,
    /// and its precedence is not portable.
    #[test]
    fn refuses_build_metadata() {
        assert!(parse("26.8.22+build.5").is_err());
        assert!(parse("26.8.22-hotfix.1+build.5").is_err());

        // The portability claim, pinned against the real implementation rather
        // than asserted from the specification. Spec §10 says precedence MUST
        // ignore build metadata; this crate orders by it. A release ordering
        // that depends on which of those is authoritative is one where this
        // pipeline and a consumer resolving the same two versions can
        // legitimately disagree — so neither is relied on, and the shape is
        // refused instead.
        let left = Version::parse("26.8.22+a").expect("valid semver");
        let right = Version::parse("26.8.22+b").expect("valid semver");
        assert_eq!(
            left.cmp(&right),
            std::cmp::Ordering::Less,
            "the crate orders by build metadata, which the specification says to ignore"
        );
    }

    /// A release candidate is an ordinary release version with a prerelease
    /// segment, and nothing here treats it as a lesser one.
    ///
    /// It once was: this crate held a `tap_follows` rule that answered false
    /// for `-rc.N`, because the Homebrew tap's own workflows refused that
    /// shape. The tap accepts it now — see `homebrew-navigator#9` — so the rule
    /// is gone and every publishable version reaches every publishing surface.
    #[test]
    fn a_release_candidate_is_a_release_version() {
        assert!(is_release_version("26.8.30-rc.1"));
        let version = parse("26.8.30-rc.1").expect("a release version");
        assert!(!version.pre.is_empty(), "an rc is a semver prerelease");
    }

    /// The prerelease flag does not separate a hotfix from a release candidate,
    /// and no consumer of it should expect it to.
    ///
    /// `-hotfix.N` and `-rc.N` are both semver prereleases and GitHub flags
    /// both as Pre-release. That flag has exactly one job — stopping GitHub
    /// reporting the release as "Latest" — and it is not a shape
    /// discriminator. A gate that needs to tell the two apart cannot be built
    /// on it, which is the mistake this test exists to keep failing.
    #[test]
    fn the_prerelease_flag_does_not_separate_a_hotfix_from_a_release_candidate() {
        let hotfix = parse("26.8.26-hotfix.1").expect("a release version");
        let candidate = parse("26.8.30-rc.1").expect("a release version");

        assert_eq!(
            hotfix.pre.is_empty(),
            candidate.pre.is_empty(),
            "both are prereleases, so `prerelease` distinguishes neither shape"
        );
    }

    /// The tomorrow-base rule, as a consequence rather than a rule. Nothing
    /// here knows what a hotfix is; the comparator does the whole job.
    #[test]
    fn a_prerelease_sorts_below_its_own_base_and_above_the_previous_release() {
        let ordered = [
            "26.8.21",
            "26.8.22-hotfix.3",
            "26.8.22-hotfix.21",
            "26.8.22",
            "26.8.23-hotfix.1",
            "26.8.23",
        ];
        let parsed: Vec<Version> = ordered
            .iter()
            .map(|text| parse(text).expect("admissible"))
            .collect();

        for pair in parsed.windows(2) {
            assert!(pair[0] < pair[1], "{} must sort below {}", pair[0], pair[1]);
        }
    }

    /// `git tag --sort=-v:refname` gets this pair backwards, which is why
    /// [`highest_release`] parses every candidate instead of trusting a sort.
    #[test]
    fn the_highest_release_is_the_base_not_its_prerelease() {
        let tags = vec![
            "26.8.20".to_string(),
            "26.8.22-hotfix.22".to_string(),
            "26.8.22".to_string(),
            "26.8.21-hotfix.12".to_string(),
        ];
        assert_eq!(
            highest_release(&tags),
            Some(Version::parse("26.8.22").expect("valid")),
            "a base version is newer than its own prerelease"
        );
    }

    /// Refs that are not releases cannot raise the bar, and a legacy
    /// four-component tag cannot block a cut.
    #[test]
    fn non_release_and_legacy_tags_are_discarded() {
        let tags = vec![
            "latest".to_string(),
            "buildcache".to_string(),
            "26.8.19.4".to_string(),
            "26.8.20".to_string(),
        ];
        assert_eq!(
            highest_release(&tags),
            Some(Version::parse("26.8.20").expect("valid"))
        );

        assert_eq!(highest_release(&["latest".to_string()]), None);
        assert_eq!(highest_release(&[]), None);
    }

    /// The three outcomes, because a caller that cannot tell "nothing to do"
    /// from "the manifest went backwards" must treat one of them wrongly.
    #[test]
    fn standing_separates_nothing_to_do_from_a_regression() {
        let released = parse("26.8.22").expect("valid");

        assert_eq!(
            standing(&parse("26.8.23").expect("valid"), Some(&released), false),
            Standing::Releasable
        );
        assert_eq!(
            standing(&parse("26.8.22").expect("valid"), Some(&released), false),
            Standing::AlreadyReleased
        );
        assert_eq!(
            standing(&parse("26.8.21").expect("valid"), Some(&released), false),
            Standing::Regression {
                highest: released.clone()
            }
        );

        // A prerelease of the version already released is BEHIND it, not ahead.
        assert_eq!(
            standing(
                &parse("26.8.22-hotfix.1").expect("valid"),
                Some(&released),
                false
            ),
            Standing::Regression {
                highest: released.clone()
            }
        );

        // An empty repository releases whatever it is handed.
        assert_eq!(
            standing(&parse("0.1.0").expect("valid"), None, false),
            Standing::Releasable
        );
    }

    /// RE-RUNNING A RELEASE THAT DIED AFTER TAGGING MUST STILL PUBLISH. The tag
    /// already names this commit, so republishing is idempotent — and the
    /// alternative is a full re-run that skips every publishing job and reports
    /// success for having done nothing.
    #[test]
    fn a_tag_already_pointing_at_this_commit_is_still_releasable() {
        let released = parse("26.8.22").expect("valid");

        assert_eq!(
            standing(&parse("26.8.22").expect("valid"), Some(&released), true),
            Standing::Releasable
        );

        // Without that, the same pair is the ordinary post-release state of
        // `main`: the tag exists, but at an ancestor.
        assert_eq!(
            standing(&parse("26.8.22").expect("valid"), Some(&released), false),
            Standing::AlreadyReleased
        );
    }

    /// The version is read out of `[workspace.package]` and nowhere else.
    #[test]
    fn workspace_version_addresses_the_table_it_means() {
        let manifest = r#"
[workspace]
members = ["cli"]

[workspace.package]
edition = "2021"
rust-version = "1.97.0"
version = "26.8.22-hotfix.22"

[workspace.dependencies]
anyhow = { version = "1.0.99" }
semver = "1"
"#;
        assert_eq!(
            workspace_version(manifest).expect("a version"),
            "26.8.22-hotfix.22"
        );
    }

    /// A manifest whose shape moved must fail loudly rather than report a
    /// dependency pin as the release version.
    #[test]
    fn workspace_version_refuses_a_manifest_without_the_key() {
        let manifest = r#"
[workspace]
members = ["cli"]

[workspace.dependencies]
anyhow = { version = "1.0.99" }
"#;
        let error = workspace_version(manifest).expect_err("no workspace package version");
        assert!(error.to_string().contains("[workspace.package]"), "{error}");
    }

    /// `rust-version` lives in the same table and is not the release version.
    #[test]
    fn workspace_version_is_not_the_rust_version() {
        let manifest = r#"
[workspace.package]
rust-version = "1.97.0"
version = "26.8.22"
"#;
        assert_eq!(workspace_version(manifest).expect("a version"), "26.8.22");
    }

    fn utc(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0)
            .single()
            .expect("a valid calendar date")
    }

    /// The convention: two-digit year, unpadded month, unpadded day.
    #[test]
    fn today_tag_formats_the_yy_m_d_convention() {
        assert_eq!(today_tag(utc(2026, 8, 22)), "26.8.22");
        // Single-digit month AND day must not gain a leading zero — that
        // shape is exactly what `parse` refuses.
        assert_eq!(today_tag(utc(2026, 1, 5)), "26.1.5");
        assert_eq!(today_tag(utc(2099, 12, 31)), "99.12.31");
    }

    /// Every date this function can format is itself an admissible release
    /// version — the contract [`default_tag`] leans on to skip re-parsing.
    #[test]
    fn today_tag_always_parses() {
        for date in [utc(2026, 8, 22), utc(2026, 1, 1), utc(2030, 12, 31)] {
            assert!(parse(&today_tag(date)).is_ok());
        }
    }

    /// An empty repository has nothing to be behind: today's date is the
    /// release.
    #[test]
    fn default_tag_is_releasable_with_no_tags_at_all() {
        assert_eq!(
            default_tag(utc(2026, 8, 22), &[]),
            Some(parse("26.8.22").expect("valid"))
        );
    }

    /// Today's date is newer than everything already released: releasable.
    #[test]
    fn default_tag_is_releasable_past_every_release() {
        let tags = vec!["26.8.20".to_string(), "26.8.21-hotfix.3".to_string()];
        assert_eq!(
            default_tag(utc(2026, 8, 22), &tags),
            Some(parse("26.8.22").expect("valid"))
        );
    }

    /// A release already exists for today: nothing to cut, and it is not an
    /// error — this is the ordinary state of asking twice in one day.
    #[test]
    fn default_tag_is_none_when_today_is_already_released() {
        let tags = vec!["26.8.21".to_string(), "26.8.22".to_string()];
        assert_eq!(default_tag(utc(2026, 8, 22), &tags), None);
    }

    /// A LATER version is already released than today's date would name —
    /// the "or later" half of the rule. Still not an error: there is nothing
    /// today's date would publish that is not already superseded.
    #[test]
    fn default_tag_is_none_when_a_later_version_is_already_released() {
        let tags = vec!["26.8.23".to_string()];
        assert_eq!(default_tag(utc(2026, 8, 22), &tags), None);
    }

    /// Non-release refs cannot raise the bar today's date has to clear.
    #[test]
    fn default_tag_ignores_non_release_tags() {
        let tags = vec!["latest".to_string(), "buildcache".to_string()];
        assert_eq!(
            default_tag(utc(2026, 8, 22), &tags),
            Some(parse("26.8.22").expect("valid"))
        );
    }
}
