//! The citation apparatus' closed vocabularies (#890): what class of
//! thing an Authority is, and what a matter did with it.
//!
//! Both are ported rather than re-derived. The disposition taxonomy in
//! particular was earned through real use on a surveyed surface, and a
//! worse home-grown version would lose the distinctions that make it
//! useful — above all the distinction between "we relied on this" and
//! "we looked at this and chose not to".

/// What class of thing an [Authority] is.
///
/// Authority is deliberately **not case-shaped**. The research corpus
/// grades statutes, regulations, and administrative proceedings alongside
/// cases, each carrying its own source-strength grade, so a statute must
/// be a first-class authority rather than a case record with its fields
/// bent to fit (#896).
///
/// [Authority]: https://www.neonlaw.com/glossary
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityClass {
    /// A judicial decision — the reported or slip opinion.
    CaseLaw,
    /// An enacted statute, in a code or session law.
    Statute,
    /// A promulgated regulation or rule.
    Regulation,
    /// An agency adjudication, order, or guidance document — the class
    /// that has no reporter and no code section, and that a case-shaped
    /// record cannot represent.
    Administrative,
    /// A treatise, practice guide, law-review article, or restatement.
    /// Persuasive only, and graded as such.
    Secondary,
}

impl AuthorityClass {
    /// Every class, in declaration order.
    pub const ALL: &'static [AuthorityClass] = &[
        AuthorityClass::CaseLaw,
        AuthorityClass::Statute,
        AuthorityClass::Regulation,
        AuthorityClass::Administrative,
        AuthorityClass::Secondary,
    ];

    /// The stored string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorityClass::CaseLaw => "case_law",
            AuthorityClass::Statute => "statute",
            AuthorityClass::Regulation => "regulation",
            AuthorityClass::Administrative => "administrative",
            AuthorityClass::Secondary => "secondary",
        }
    }

    /// Parse a stored value, or `None` when it is outside the vocabulary.
    #[must_use]
    pub fn parse(value: &str) -> Option<AuthorityClass> {
        Self::ALL.iter().copied().find(|c| c.as_str() == value)
    }

    /// True when this class carries binding force in its own
    /// jurisdiction, as opposed to persuasive weight only.
    ///
    /// Deliberately an exhaustive `match`: a new class must declare which
    /// side of the line it falls on rather than inheriting a default.
    #[must_use]
    pub fn is_primary(self) -> bool {
        match self {
            AuthorityClass::CaseLaw
            | AuthorityClass::Statute
            | AuthorityClass::Regulation
            | AuthorityClass::Administrative => true,
            AuthorityClass::Secondary => false,
        }
    }
}

/// What a matter did with an authority — the closed disposition
/// taxonomy, ported whole from the surveyed surface that earned it.
///
/// # The load-bearing rule
///
/// Several of these values are **firm reasoning**: they record what the
/// firm considered and chose not to rely on. A client who sees
/// "reviewed, not used" learns the firm's strategic assessment of their
/// own matter. That is a disclosure of work product, not merely a data
/// leak, and it is a different and worse failure than an ordinary
/// visibility bug.
///
/// So [`Disposition::is_firm_reasoning`] gates them, and no
/// client-facing section allowlist may ever contain one. `rules` owns
/// that predicate so the rule is stated once and every surface asks the
/// same question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Cited and relied on as stated.
    ReliedOn,
    /// Relied on, but with a stated limit — distinguished on its facts,
    /// narrowed to a holding, or good only for part of the proposition.
    ReliedOnWithQualification,
    /// Read and deliberately not used. **Firm reasoning.**
    ReviewedNotUsed,
    /// In the record as an exhibit, but not relied on for any
    /// proposition. **Firm reasoning.**
    RecordExhibitNotReliedOn,
    /// Captured as an exhibit but never quoted. **Firm reasoning.**
    CapturedExhibitNotQuoted,
    /// Being watched — a pending appeal, a proposed rule — and not relied
    /// on. **Firm reasoning.**
    MonitoringNotReliedOn,
    /// The authority is cited but its source artifact has not been
    /// obtained yet. A work state, not a conclusion.
    SourcePending,
    /// Under review; nobody has reached a disposition. A work state, not
    /// a conclusion.
    OpenReview,
}

impl Disposition {
    /// Every disposition, in declaration order.
    pub const ALL: &'static [Disposition] = &[
        Disposition::ReliedOn,
        Disposition::ReliedOnWithQualification,
        Disposition::ReviewedNotUsed,
        Disposition::RecordExhibitNotReliedOn,
        Disposition::CapturedExhibitNotQuoted,
        Disposition::MonitoringNotReliedOn,
        Disposition::SourcePending,
        Disposition::OpenReview,
    ];

    /// The stored string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Disposition::ReliedOn => "relied-on",
            Disposition::ReliedOnWithQualification => "relied-on-with-qualification",
            Disposition::ReviewedNotUsed => "reviewed-not-used",
            Disposition::RecordExhibitNotReliedOn => "record-exhibit-not-relied-on",
            Disposition::CapturedExhibitNotQuoted => "captured-exhibit-not-quoted",
            Disposition::MonitoringNotReliedOn => "monitoring-not-relied-on",
            Disposition::SourcePending => "source-pending",
            Disposition::OpenReview => "open-review",
        }
    }

    /// Parse a stored value, or `None` when it is outside the vocabulary.
    #[must_use]
    pub fn parse(value: &str) -> Option<Disposition> {
        Self::ALL.iter().copied().find(|d| d.as_str() == value)
    }

    /// True when this disposition records **firm reasoning** — what the
    /// firm considered and chose not to rely on.
    ///
    /// Never expose one of these on a client-facing surface. The
    /// "not-used" and "monitoring" values disclose the firm's
    /// deliberative process about the client's own matter; see
    /// [`Disposition::client_visible`], which is the only sanctioned way
    /// to build a client allowlist.
    ///
    /// Deliberately an exhaustive `match`: a new disposition must declare
    /// whether it is firm reasoning rather than defaulting to safe, which
    /// is exactly the mistake that would leak the next one.
    #[must_use]
    pub fn is_firm_reasoning(self) -> bool {
        match self {
            Disposition::ReviewedNotUsed
            | Disposition::RecordExhibitNotReliedOn
            | Disposition::CapturedExhibitNotQuoted
            | Disposition::MonitoringNotReliedOn => true,
            // What the firm actually relied on is the client's own work
            // product, and the two work states disclose no assessment.
            Disposition::ReliedOn
            | Disposition::ReliedOnWithQualification
            | Disposition::SourcePending
            | Disposition::OpenReview => false,
        }
    }

    /// The dispositions a client-facing section may list — the complement
    /// of [`Disposition::is_firm_reasoning`], derived rather than
    /// hand-written so the two can never drift apart.
    ///
    /// A hand-maintained allowlist is precisely how the next disposition
    /// leaks: someone adds a variant, the compiler forces them to answer
    /// `is_firm_reasoning`, and a second literal list silently keeps its
    /// old contents.
    #[must_use]
    pub fn client_visible() -> Vec<Disposition> {
        Self::ALL
            .iter()
            .copied()
            .filter(|d| !d.is_firm_reasoning())
            .collect()
    }
}

/// One of the three independent axes a citation is verified along.
///
/// The surveyed corpus decomposes verification rather than treating it as
/// a boolean, and the decomposition is the point: a single "verified"
/// flag hides [`Axis::Proposition`], which is the axis that catches the
/// failure that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Is the citation real and correctly formatted?
    Citation,
    /// Is the quoted text accurate to the source?
    Quote,
    /// Does the source actually support the assertion it is cited for?
    ///
    /// A failing proposition check means someone cited a real case,
    /// quoted it accurately, for something it does not say. Neither of
    /// the other two axes can catch that.
    Proposition,
}

impl Axis {
    /// Every axis, in declaration order.
    pub const ALL: &'static [Axis] = &[Axis::Citation, Axis::Quote, Axis::Proposition];

    /// The stored string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::Citation => "citation",
            Axis::Quote => "quote",
            Axis::Proposition => "proposition",
        }
    }

    /// Parse a stored value, or `None` when it is outside the vocabulary.
    #[must_use]
    pub fn parse(value: &str) -> Option<Axis> {
        Self::ALL.iter().copied().find(|a| a.as_str() == value)
    }
}

/// The state of one [`Axis`] of one verification.
///
/// [`AxisStatus::Unverified`] is the **default and the only safe seed**.
/// Backfilling an axis as passing overclaims a verification, which is
/// worse than having none: it asserts that a licensed human checked
/// something nobody checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisStatus {
    /// Nobody has checked this axis. The seed state.
    #[default]
    Unverified,
    /// A licensed human checked it and it holds.
    Verified,
    /// A licensed human checked it and it does not hold.
    Rejected,
    /// It was verified, but against a draft revision that has since
    /// moved. The check may still be right — nothing records that anyone
    /// confirmed it against the current text.
    Stale,
}

impl AxisStatus {
    /// Every status, in declaration order.
    pub const ALL: &'static [AxisStatus] = &[
        AxisStatus::Unverified,
        AxisStatus::Verified,
        AxisStatus::Rejected,
        AxisStatus::Stale,
    ];

    /// The stored string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AxisStatus::Unverified => "unverified",
            AxisStatus::Verified => "verified",
            AxisStatus::Rejected => "rejected",
            AxisStatus::Stale => "stale",
        }
    }

    /// Parse a stored value, or `None` when it is outside the vocabulary.
    #[must_use]
    pub fn parse(value: &str) -> Option<AxisStatus> {
        Self::ALL.iter().copied().find(|s| s.as_str() == value)
    }

    /// True when a draft edit should carry this status to
    /// [`AxisStatus::Stale`].
    ///
    /// Only a status that makes a *claim about the text* goes stale. An
    /// unverified axis claims nothing, and an already-stale one has
    /// nowhere further to go — carrying either would manufacture a
    /// transition that did not happen and pollute the staleness rate the
    /// telemetry measures.
    ///
    /// Deliberately an exhaustive `match`: a new status must declare
    /// whether a moving draft invalidates it.
    #[must_use]
    pub fn goes_stale_on_edit(self) -> bool {
        match self {
            AxisStatus::Verified | AxisStatus::Rejected => true,
            AxisStatus::Unverified | AxisStatus::Stale => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthorityClass, Disposition};

    #[test]
    fn every_class_round_trips() {
        for class in AuthorityClass::ALL {
            assert_eq!(AuthorityClass::parse(class.as_str()), Some(*class));
        }
        assert_eq!(AuthorityClass::parse("bogus"), None);
    }

    #[test]
    fn authority_is_not_case_shaped() {
        // The requirement hoisted from #896: a statute or an
        // administrative proceeding is a first-class authority, not a
        // case record with its fields bent to fit.
        for class in [
            AuthorityClass::Statute,
            AuthorityClass::Regulation,
            AuthorityClass::Administrative,
        ] {
            assert!(class.is_primary(), "{} binds", class.as_str());
            assert_ne!(class, AuthorityClass::CaseLaw);
        }
        assert!(!AuthorityClass::Secondary.is_primary());
    }

    #[test]
    fn every_disposition_round_trips_on_the_ported_spelling() {
        // The stored spellings are the donor surface's, hyphenated. A
        // silent re-spelling would strand every ported row.
        let spellings: Vec<&str> = Disposition::ALL.iter().map(|d| d.as_str()).collect();
        assert_eq!(
            spellings,
            [
                "relied-on",
                "relied-on-with-qualification",
                "reviewed-not-used",
                "record-exhibit-not-relied-on",
                "captured-exhibit-not-quoted",
                "monitoring-not-relied-on",
                "source-pending",
                "open-review",
            ]
        );
        for d in Disposition::ALL {
            assert_eq!(Disposition::parse(d.as_str()), Some(*d));
        }
        assert_eq!(Disposition::parse("relied_on"), None);
    }

    /// The load-bearing rule. A client seeing "reviewed, not used" learns
    /// the firm's strategic assessment of their own matter — a disclosure
    /// of work product, not merely a data leak.
    #[test]
    fn no_not_used_or_monitoring_disposition_is_ever_client_visible() {
        let allowed = Disposition::client_visible();
        for d in [
            Disposition::ReviewedNotUsed,
            Disposition::RecordExhibitNotReliedOn,
            Disposition::CapturedExhibitNotQuoted,
            Disposition::MonitoringNotReliedOn,
        ] {
            assert!(d.is_firm_reasoning(), "{} is firm reasoning", d.as_str());
            assert!(
                !allowed.contains(&d),
                "{} must never reach a client allowlist",
                d.as_str()
            );
        }
    }

    #[test]
    fn the_client_allowlist_is_derived_and_cannot_drift() {
        // Not a second hand-written list: every disposition is on exactly
        // one side, and the allowlist is the complement of the predicate.
        let allowed = Disposition::client_visible();
        assert_eq!(
            allowed,
            [
                Disposition::ReliedOn,
                Disposition::ReliedOnWithQualification,
                Disposition::SourcePending,
                Disposition::OpenReview,
            ]
        );
        assert_eq!(
            allowed.len()
                + Disposition::ALL
                    .iter()
                    .filter(|d| d.is_firm_reasoning())
                    .count(),
            Disposition::ALL.len(),
            "every disposition falls on exactly one side"
        );
    }
}

#[cfg(test)]
mod axis_tests {
    use super::{Axis, AxisStatus};

    #[test]
    fn the_three_axes_are_independent_and_round_trip() {
        assert_eq!(Axis::ALL.len(), 3, "a boolean would hide the third");
        for axis in Axis::ALL {
            assert_eq!(Axis::parse(axis.as_str()), Some(*axis));
        }
        assert_eq!(Axis::parse("overall"), None, "there is no aggregate axis");
    }

    #[test]
    fn unverified_is_the_default_because_overclaiming_is_worse_than_nothing() {
        assert_eq!(AxisStatus::default(), AxisStatus::Unverified);
        for status in AxisStatus::ALL {
            assert_eq!(AxisStatus::parse(status.as_str()), Some(*status));
        }
    }

    #[test]
    fn only_a_status_that_claims_something_about_the_text_goes_stale() {
        // A moving draft invalidates a claim; it cannot invalidate the
        // absence of one.
        assert!(AxisStatus::Verified.goes_stale_on_edit());
        assert!(AxisStatus::Rejected.goes_stale_on_edit());
        assert!(!AxisStatus::Unverified.goes_stale_on_edit());
        assert!(
            !AxisStatus::Stale.goes_stale_on_edit(),
            "re-staling would manufacture a transition that did not happen"
        );
    }
}
