//! The declared document `kind` — the single frontmatter discriminator
//! that names what a Neon Law Navigator markdown file *is*.
//!
//! Every file the workspace lints belongs to one [`crate::engine::DocumentKind`]
//! family, and a declared `kind:` key is the **only** thing that names it:
//! the classifier reads the key directly and does not infer the family
//! from a file's structure or path. A file that declares no `kind:` is
//! plain prose Markdown; [`crate::S104MissingKind`] catches a file that
//! carries notation/event structure yet forgot to declare one.
//!
//! The vocabulary is a small, closed enum extended deliberately as the
//! firm's practice areas grow. Most values name notation-family kinds (a
//! legal template that becomes a running Notation); some name content
//! pages (events, posts, workshops); and the rest name
//! **matter dashboard kinds** — the page types an attorney composes,
//! whose section skeletons live in [`crate::dashboard`].

/// A recognized value of the `kind:` frontmatter key.
///
/// An unrecognized string is **not** a `Kind` — [`Kind::parse`] returns
/// `None`, the file classifies as plain Markdown, and
/// [`crate::S103KindEnum`] flags the bad value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A letter the firm sends on a client's behalf (demand, notice,
    /// settlement, closing).
    Letter,
    /// A document filed with a government body (formation, annual report,
    /// tax return, application).
    Filing,
    /// A last will and testament.
    Will,
    /// A trust instrument.
    Trust,
    /// An advance directive — a health-care directive or a durable
    /// financial power of attorney.
    Directive,
    /// A private agreement between the client and a third party
    /// (employment, contractor, LLC operating agreement).
    Agreement,
    /// The engagement that opens a matter — a lawyer creates it on the
    /// Project like any other notation (opening the Project does not open
    /// it) and it is the one kind the self-serve doors accept as a
    /// matter's first notation (see [`Kind::opens_a_matter`]). The shipped
    /// sample is the engagement letter.
    Onboarding,
    /// The firm-signed letter that **closes a matter** — the mirror of
    /// [`Kind::Onboarding`] (see [`Kind::closes_a_matter`]). Distinct from
    /// the general [`Kind::Letter`]: every offboarding letter is a letter,
    /// but `Kind::Letter` alone must not clear a matter's offboarding flag,
    /// or any demand or notice letter would silently close it out.
    Offboarding,
    /// An analytical work product the firm delivers — a review memo or
    /// opinion, not an executed instrument.
    Memo,
    /// A dated public event page under `server/content/events/`.
    Event,
    /// A published blog post under `web/content/blog/`.
    Post,
    /// A public workshop / teaching page under
    /// `web/content/workshops/`.
    Workshop,
    /// An engineering intake notation under `templates/github/` — the
    /// questionnaire that gathers what a GitHub issue or pull request
    /// needs before it is opened, and the body that renders it.
    ///
    /// It borrows the questionnaire grammar but is **not** a legal
    /// instrument: it binds to no respondent, carries no jurisdiction or
    /// confidentiality classification, and never reaches lawyer review.
    /// [`Kind::Github`] is therefore not [`Kind::is_notation`]; the rules
    /// it is held to are the questionnaire-grammar subset plus `N119`.
    Github,
    /// A recorded sitting transcript filed on a matter (the Northstar
    /// estate intake's `document_intake__transcript` step). **Asset-lane
    /// only** — a document classification for an `assets` row, not a
    /// notation-template kind; declaring `kind: transcript` in a
    /// template's frontmatter is nonsensical, and
    /// [`Kind::valid_for`]`(`[`Lane::Template`]`)` rejects it there. Not
    /// [`Kind::is_notation`] and not [`Kind::carries_questionnaire`].
    Transcript,
    /// An inbound contract a client uploads for review (the contract-review
    /// walk's `document_intake__*` step). Same asset-lane-only caveat as
    /// [`Kind::Transcript`].
    InboundContract,
    /// The issued USCIS Form N-550, filed when a naturalization matter's
    /// `document_intake__certificate_of_naturalization` step receives it.
    /// Same asset-lane-only caveat as [`Kind::Transcript`].
    CertificateOfNaturalization,
    /// A filed artifact nobody has classified yet — an inbound email
    /// attachment, a blank-kind lawyer upload, an expunge tombstone.
    ///
    /// **Asset-lane only**, and a real variant rather than an absent
    /// value on purpose: it is the `NOT NULL DEFAULT` the `assets.kind`
    /// column carries, so every exhaustive match over [`Kind`] must
    /// declare which side of its line an unclassified document falls on
    /// instead of silently treating "no kind" as "no opinion".
    Unclassified,

    // --- Matter dashboard kinds (#896). Each names a page type an
    // attorney composes, and each carries a section skeleton in
    // [`crate::dashboard`]. None is a notation template, so none carries
    // a questionnaire or opens a matter.
    /// The dominant layout in the surveyed corpus: a filterable rail of
    /// items, a detail pane, and a per-item status. Document review,
    /// discovery response, and citation checking are specialisations.
    ReviewQueueWorkbench,
    /// Drafted assertion on one side, source page on the other, both
    /// marked, with a status setter (#891, #893).
    VerifierSplitView,
    /// Multi-workstream state: metrics, per-workstream cards, and the
    /// decisions only the principal can make.
    MatterStatusConsole,
    /// Dated rows with hard/soft classification, provenance confidence,
    /// and per-row calendar export (#539, #688).
    DocketDeadlineBoard,
    /// A draft with a block-structured body and phrase-anchored comments.
    DocumentWorkbench,
    /// A searchable set of authorities, each with citation, holding, the
    /// verbatim quote, and its position for this matter (#890).
    AuthorityLibrary,
    /// Numbered request/response pairs with objection tracking.
    DiscoveryCockpit,
    /// Ordered stages with argument points, authorities, and anticipated
    /// questions.
    HearingConsole,
    /// A linear, non-interactive handoff page: what is included, what is
    /// excluded, downloads, and a provenance statement.
    DeliverablePackage,
    /// Engagement letter, invoice, and prebill review with per-entry
    /// findings.
    EngagementBillingRecords,
}

impl Kind {
    /// Every recognized kind, in declaration order.
    pub const ALL: &'static [Kind] = &[
        Kind::Letter,
        Kind::Filing,
        Kind::Will,
        Kind::Trust,
        Kind::Directive,
        Kind::Agreement,
        Kind::Onboarding,
        Kind::Offboarding,
        Kind::Memo,
        Kind::Event,
        Kind::Post,
        Kind::Workshop,
        Kind::Github,
        Kind::Transcript,
        Kind::InboundContract,
        Kind::CertificateOfNaturalization,
        Kind::Unclassified,
        Kind::ReviewQueueWorkbench,
        Kind::VerifierSplitView,
        Kind::MatterStatusConsole,
        Kind::DocketDeadlineBoard,
        Kind::DocumentWorkbench,
        Kind::AuthorityLibrary,
        Kind::DiscoveryCockpit,
        Kind::HearingConsole,
        Kind::DeliverablePackage,
        Kind::EngagementBillingRecords,
    ];

    /// The frontmatter string this kind is written as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Letter => "letter",
            Kind::Filing => "filing",
            Kind::Will => "will",
            Kind::Trust => "trust",
            Kind::Directive => "directive",
            Kind::Agreement => "agreement",
            Kind::Onboarding => "onboarding",
            Kind::Offboarding => "offboarding",
            Kind::Memo => "memo",
            Kind::Event => "event",
            Kind::Post => "post",
            Kind::Workshop => "workshop",
            Kind::Github => "github",
            Kind::Transcript => "transcript",
            Kind::InboundContract => "inbound_contract",
            Kind::CertificateOfNaturalization => "certificate_of_naturalization",
            Kind::Unclassified => "unclassified",
            Kind::ReviewQueueWorkbench => "review_queue_workbench",
            Kind::VerifierSplitView => "verifier_split_view",
            Kind::MatterStatusConsole => "matter_status_console",
            Kind::DocketDeadlineBoard => "docket_deadline_board",
            Kind::DocumentWorkbench => "document_workbench",
            Kind::AuthorityLibrary => "authority_library",
            Kind::DiscoveryCockpit => "discovery_cockpit",
            Kind::HearingConsole => "hearing_console",
            Kind::DeliverablePackage => "deliverable_package",
            Kind::EngagementBillingRecords => "engagement_billing_records",
        }
    }

    /// A one-line, human-readable summary of the kind — the surface an
    /// editor completion shows next to each value.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Kind::Letter => "A letter the firm sends on the client's behalf",
            Kind::Filing => "A document filed with a government body",
            Kind::Will => "A last will and testament",
            Kind::Trust => "A trust instrument",
            Kind::Directive => "An advance health-care or durable financial directive",
            Kind::Agreement => "A private agreement (employment, contractor, LLC operating)",
            Kind::Onboarding => "The engagement that opens a matter — one instrument or a bundle",
            Kind::Offboarding => "The firm-signed letter that closes a matter",
            Kind::Memo => "An analytical work product (a review memo or opinion)",
            Kind::Event => "A public event page (web/content/events/)",
            Kind::Post => "A published blog post (web/content/blog/)",
            Kind::Workshop => "A public workshop / teaching page (web/content/workshops/)",
            Kind::Github => {
                "An engineering intake notation that opens a GitHub issue or pull request \
                 (templates/github/)"
            }
            Kind::Transcript => "A recorded sitting transcript filed on a matter (asset-lane only)",
            Kind::InboundContract => {
                "An inbound contract a client uploaded for review (asset-lane only)"
            }
            Kind::CertificateOfNaturalization => {
                "An issued USCIS Certificate of Naturalization (Form N-550, asset-lane only)"
            }
            Kind::Unclassified => "A filed artifact nobody has classified yet (asset-lane only)",
            Kind::ReviewQueueWorkbench => {
                "A matter dashboard: a filterable item rail, a detail pane, and a per-item status"
            }
            Kind::VerifierSplitView => {
                "A matter dashboard: drafted assertion beside the source page, both marked"
            }
            Kind::MatterStatusConsole => {
                "A matter dashboard: workstream state, key dates, and the principal's decisions"
            }
            Kind::DocketDeadlineBoard => {
                "A matter dashboard: dated rows with hard/soft classification and calendar export"
            }
            Kind::DocumentWorkbench => {
                "A matter dashboard: a draft body with phrase-anchored comments"
            }
            Kind::AuthorityLibrary => {
                "A matter dashboard: authorities with citation, holding, quote, and position"
            }
            Kind::DiscoveryCockpit => {
                "A matter dashboard: numbered request/response pairs with objection tracking"
            }
            Kind::HearingConsole => {
                "A matter dashboard: ordered stages with argument points and anticipated questions"
            }
            Kind::DeliverablePackage => {
                "A matter deliverable: a linear handoff page of inclusions, exclusions, and files"
            }
            Kind::EngagementBillingRecords => {
                "A matter dashboard: engagement letter, invoice, and prebill review"
            }
        }
    }

    /// Parse a frontmatter value into a `Kind`, or `None` when it is not
    /// one of the recognized vocabulary values.
    #[must_use]
    pub fn parse(value: &str) -> Option<Kind> {
        Self::ALL.iter().copied().find(|k| k.as_str() == value)
    }

    /// True when a notation of this kind is the **engagement that opens a
    /// matter** — the document whose absence means a Project was never
    /// really opened. `Onboarding` covers both a single-instrument
    /// engagement letter and the intake-driven engagement that opens a
    /// bundle of instruments. Callers that ask "does this matter have its
    /// engagement yet?" (`portal::admin::matter_flags`) key off this rather
    /// than a template's `code`, so a new engagement template is classified
    /// by what it *is*, not by what it happens to be named.
    ///
    /// Deliberately an exhaustive `match`: adding a [`Kind`] fails to
    /// compile until it declares which side of this line it falls on.
    #[must_use]
    pub fn opens_a_matter(self) -> bool {
        match self {
            Kind::Onboarding => true,
            Kind::Letter
            | Kind::Filing
            | Kind::Will
            | Kind::Trust
            | Kind::Directive
            | Kind::Agreement
            | Kind::Offboarding
            | Kind::Memo
            | Kind::Event
            | Kind::Post
            | Kind::Workshop
            | Kind::Github
            | Kind::Transcript
            | Kind::InboundContract
            | Kind::CertificateOfNaturalization
            | Kind::Unclassified
            | Kind::ReviewQueueWorkbench
            | Kind::VerifierSplitView
            | Kind::MatterStatusConsole
            | Kind::DocketDeadlineBoard
            | Kind::DocumentWorkbench
            | Kind::AuthorityLibrary
            | Kind::DiscoveryCockpit
            | Kind::HearingConsole
            | Kind::DeliverablePackage
            | Kind::EngagementBillingRecords => false,
        }
    }

    /// True when a notation of this kind is the **letter that closes a
    /// matter** — the mirror of [`Kind::opens_a_matter`]. Callers that ask
    /// "does this matter have its offboarding letter yet?"
    /// (`store::projects::template_closes_a_matter`) key off this rather
    /// than a template's `code`, so a bespoke closing letter is classified
    /// by what it *is*, not by what it happens to be named.
    ///
    /// `Kind::Letter` alone does **not** close a matter — an ordinary
    /// demand, notice, or settlement letter must not be mistaken for the
    /// one that ends the representation.
    ///
    /// Deliberately an exhaustive `match`: adding a [`Kind`] fails to
    /// compile until it declares which side of this line it falls on.
    #[must_use]
    pub fn closes_a_matter(self) -> bool {
        match self {
            Kind::Offboarding => true,
            Kind::Letter
            | Kind::Filing
            | Kind::Will
            | Kind::Trust
            | Kind::Directive
            | Kind::Agreement
            | Kind::Onboarding
            | Kind::Memo
            | Kind::Event
            | Kind::Post
            | Kind::Workshop
            | Kind::Github
            | Kind::Transcript
            | Kind::InboundContract
            | Kind::CertificateOfNaturalization
            | Kind::Unclassified
            | Kind::ReviewQueueWorkbench
            | Kind::VerifierSplitView
            | Kind::MatterStatusConsole
            | Kind::DocketDeadlineBoard
            | Kind::DocumentWorkbench
            | Kind::AuthorityLibrary
            | Kind::DiscoveryCockpit
            | Kind::HearingConsole
            | Kind::DeliverablePackage
            | Kind::EngagementBillingRecords => false,
        }
    }

    /// True when this kind names a **matter dashboard** — a page type an
    /// attorney composes from registered sections, whose skeleton lives
    /// in [`crate::dashboard::skeleton`].
    ///
    /// A dashboard kind is never notation-family: it declares no
    /// questionnaire, opens no matter, and becomes no instrument. It
    /// drives validation and scaffolding only — never rendering (#690).
    #[must_use]
    pub fn is_dashboard(self) -> bool {
        matches!(
            self,
            Kind::ReviewQueueWorkbench
                | Kind::VerifierSplitView
                | Kind::MatterStatusConsole
                | Kind::DocketDeadlineBoard
                | Kind::DocumentWorkbench
                | Kind::AuthorityLibrary
                | Kind::DiscoveryCockpit
                | Kind::HearingConsole
                | Kind::DeliverablePackage
                | Kind::EngagementBillingRecords
        )
    }

    /// True when this kind is a notation-family kind — a legal template
    /// that declares a questionnaire/workflow and becomes a Notation — as
    /// opposed to a content page (event/post/workshop).
    #[must_use]
    pub fn is_notation(self) -> bool {
        matches!(
            self,
            Kind::Letter
                | Kind::Filing
                | Kind::Will
                | Kind::Trust
                | Kind::Directive
                | Kind::Agreement
                | Kind::Onboarding
                | Kind::Offboarding
                | Kind::Memo
        )
    }

    /// True when a file of this kind legitimately declares the
    /// `questionnaire:` machine.
    ///
    /// Every notation-family kind does, and so does [`Kind::Github`],
    /// which drives an engineering artifact through the same questionnaire
    /// grammar without being a legal instrument. A content page
    /// (`post`, `workshop`) never does — [`crate::S104MissingKind`]
    /// keys on this to flag a content page carrying a copied
    /// `questionnaire:`/`workflow:` block, which would otherwise skip every
    /// structural rule in silence.
    #[must_use]
    pub fn carries_questionnaire(self) -> bool {
        self.is_notation() || self == Kind::Github
    }

    /// True when this kind may be declared in `lane`.
    ///
    /// One vocabulary spans both of Navigator's document lanes, but not
    /// every value makes sense in both. `kind: transcript` in a template's
    /// frontmatter is nonsensical — a transcript is a byte artifact
    /// somebody files, not text the firm drafts — and `kind: workshop` on
    /// an `assets` row is equally nonsensical, because a teaching page is
    /// never a document filed on a matter.
    ///
    /// The split is what lets each lane's write boundary stay closed:
    /// [`Lane::Template`] backs `VALID`, `S103`, and the LSP `kind:`
    /// completion, while [`Lane::Asset`] backs `store::documents`'
    /// ingest boundary.
    ///
    /// Deliberately an exhaustive `match`: adding a [`Kind`] fails to
    /// compile until it declares which lanes it belongs to.
    #[must_use]
    pub fn valid_for(self, lane: Lane) -> bool {
        match lane {
            // Everything an author can write into a Markdown file's
            // frontmatter: the notation family, the content pages, the
            // GitHub intake, and the matter dashboards.
            Lane::Template => match self {
                Kind::Letter
                | Kind::Filing
                | Kind::Will
                | Kind::Trust
                | Kind::Directive
                | Kind::Agreement
                | Kind::Onboarding
                | Kind::Offboarding
                | Kind::Memo
                | Kind::Event
                | Kind::Post
                | Kind::Workshop
                | Kind::Github
                | Kind::ReviewQueueWorkbench
                | Kind::VerifierSplitView
                | Kind::MatterStatusConsole
                | Kind::DocketDeadlineBoard
                | Kind::DocumentWorkbench
                | Kind::AuthorityLibrary
                | Kind::DiscoveryCockpit
                | Kind::HearingConsole
                | Kind::DeliverablePackage
                | Kind::EngagementBillingRecords => true,
                Kind::Transcript
                | Kind::InboundContract
                | Kind::CertificateOfNaturalization
                | Kind::Unclassified => false,
            },
            // Everything that can be *filed on a matter*: the three
            // intake-only classifications, the unclassified default, and
            // every notation kind — a generated PDF lands in `assets`
            // carrying its owning template's declared kind (#830).
            //
            // Content pages and dashboards are excluded: neither is ever
            // a byte artifact on a matter.
            Lane::Asset => match self {
                Kind::Letter
                | Kind::Filing
                | Kind::Will
                | Kind::Trust
                | Kind::Directive
                | Kind::Agreement
                | Kind::Onboarding
                | Kind::Offboarding
                | Kind::Memo
                | Kind::Transcript
                | Kind::InboundContract
                | Kind::CertificateOfNaturalization
                | Kind::Unclassified => true,
                Kind::Event
                | Kind::Post
                | Kind::Workshop
                | Kind::Github
                | Kind::ReviewQueueWorkbench
                | Kind::VerifierSplitView
                | Kind::MatterStatusConsole
                | Kind::DocketDeadlineBoard
                | Kind::DocumentWorkbench
                | Kind::AuthorityLibrary
                | Kind::DiscoveryCockpit
                | Kind::HearingConsole
                | Kind::DeliverablePackage
                | Kind::EngagementBillingRecords => false,
            },
        }
    }
}

/// Which of Navigator's two document lanes a [`Kind`] is being declared
/// in.
///
/// The lanes are not two vocabularies — they are two *write boundaries*
/// over one vocabulary, each admitting the subset that means something
/// for it. See [`Kind::valid_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// A Markdown file in the repository — a notation template, a content
    /// page, or a matter dashboard. Its kind arrives as the `kind:`
    /// frontmatter key, so this lane backs `S103` and the LSP completion.
    Template,
    /// A byte artifact filed on a matter (an `assets` row). Its kind is
    /// chosen by whoever files it, never parsed from frontmatter, so this
    /// lane backs `store::documents`' ingest boundary.
    Asset,
}

/// The kinds a Markdown file may declare, as strings, for diagnostics and
/// completion.
///
/// This is the [`Lane::Template`] slice of the vocabulary, not the whole
/// enum: the asset-lane-only values (`transcript`, `inbound_contract`,
/// `certificate_of_naturalization`, `unclassified`) classify a filed byte
/// artifact and mean nothing in frontmatter, so `S103` rejects them and
/// the LSP does not offer them. `valid_strings_are_exactly_the_template_lane`
/// pins the two together.
pub const VALID: &[&str] = &[
    "letter",
    "filing",
    "will",
    "trust",
    "directive",
    "agreement",
    "onboarding",
    "offboarding",
    "memo",
    "event",
    "post",
    "workshop",
    "github",
    "review_queue_workbench",
    "verifier_split_view",
    "matter_status_console",
    "docket_deadline_board",
    "document_workbench",
    "authority_library",
    "discovery_cockpit",
    "hearing_console",
    "deliverable_package",
    "engagement_billing_records",
];

/// The declared `kind:` for a file's contents, when present and
/// recognized. Returns `None` when the key is absent or its value is not
/// a known kind — in the latter case the file classifies as plain
/// Markdown and [`crate::S103KindEnum`] reports the bad value.
#[must_use]
pub fn declared(contents: &str) -> Option<Kind> {
    let fm = crate::frontmatter::extract(contents)?;
    let raw = crate::frontmatter::field(fm, "kind")?;
    Kind::parse(&raw)
}

#[cfg(test)]
mod tests {
    use super::{declared, Kind, Lane};

    #[test]
    fn parse_round_trips_every_kind() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(*kind));
        }
    }

    #[test]
    fn parse_rejects_unknown_value() {
        assert_eq!(Kind::parse("bogus"), None);
        assert_eq!(Kind::parse(""), None);
    }

    #[test]
    fn valid_strings_are_exactly_the_template_lane() {
        // `VALID` (the diagnostics/completion surface) must never drift
        // from the template lane, or S103 would accept a value that means
        // nothing in frontmatter, or reject one that does.
        let template_lane: Vec<&str> = Kind::ALL
            .iter()
            .filter(|k| k.valid_for(Lane::Template))
            .map(|k| k.as_str())
            .collect();
        assert_eq!(template_lane, super::VALID);
    }

    #[test]
    fn the_asset_only_kinds_are_not_declarable_in_frontmatter() {
        // A transcript is filed, not drafted. Declaring one as a
        // template's `kind:` is the mistake this lane split exists to
        // catch, so `S103`'s accepted list must not contain it.
        for kind in [
            Kind::Transcript,
            Kind::InboundContract,
            Kind::CertificateOfNaturalization,
            Kind::Unclassified,
        ] {
            assert!(!kind.valid_for(Lane::Template), "{}", kind.as_str());
            assert!(kind.valid_for(Lane::Asset), "{}", kind.as_str());
            assert!(
                !super::VALID.contains(&kind.as_str()),
                "{} leaked into VALID",
                kind.as_str()
            );
        }
    }

    #[test]
    fn a_content_page_or_dashboard_is_never_filed_on_a_matter() {
        // The mirror of the rule above: a teaching page or a dashboard
        // skeleton is never a byte artifact somebody files on a matter,
        // so the ingest boundary must refuse it.
        for kind in [
            Kind::Event,
            Kind::Post,
            Kind::Workshop,
            Kind::Github,
            Kind::ReviewQueueWorkbench,
            Kind::AuthorityLibrary,
        ] {
            assert!(!kind.valid_for(Lane::Asset), "{}", kind.as_str());
            assert!(kind.valid_for(Lane::Template), "{}", kind.as_str());
        }
    }

    #[test]
    fn every_kind_belongs_to_at_least_one_lane() {
        // A kind valid in neither lane could never be written anywhere —
        // dead vocabulary that `S103` would still advertise.
        for kind in Kind::ALL {
            assert!(
                kind.valid_for(Lane::Template) || kind.valid_for(Lane::Asset),
                "{} belongs to no lane",
                kind.as_str()
            );
        }
    }

    #[test]
    fn notation_family_covers_every_instrument_and_excludes_content_pages() {
        for kind in [
            Kind::Letter,
            Kind::Filing,
            Kind::Will,
            Kind::Trust,
            Kind::Directive,
            Kind::Agreement,
            Kind::Onboarding,
            Kind::Offboarding,
            Kind::Memo,
        ] {
            assert!(
                kind.is_notation(),
                "{} should be notation-family",
                kind.as_str()
            );
        }
        for kind in [Kind::Event, Kind::Post, Kind::Workshop] {
            assert!(!kind.is_notation(), "{} is a content page", kind.as_str());
        }
        // A dashboard kind composes registered sections. It declares no
        // questionnaire and becomes no instrument, so it must stay out of
        // the notation family and never pick up the legal N-family rules.
        for kind in Kind::ALL.iter().filter(|k| k.is_dashboard()) {
            assert!(
                !kind.is_notation(),
                "{} is a matter dashboard, not a notation template",
                kind.as_str(),
            );
            assert!(!kind.carries_questionnaire(), "{}", kind.as_str());
            assert!(!kind.opens_a_matter(), "{}", kind.as_str());
        }
        // `github` borrows the questionnaire grammar but is an engineering
        // artifact, not a legal instrument — it must stay out of the
        // notation family so it never picks up the legal N-family rules.
        assert!(
            !Kind::Github.is_notation(),
            "github is not a legal instrument"
        );
        // The asset-lane-only kinds are not notation-template kinds either
        // — nobody declares `kind: transcript` in a template's frontmatter.
        for kind in [
            Kind::Transcript,
            Kind::InboundContract,
            Kind::CertificateOfNaturalization,
        ] {
            assert!(
                !kind.is_notation(),
                "{} is an asset-lane classification, not a notation template",
                kind.as_str()
            );
        }
    }

    #[test]
    fn questionnaire_carrying_kinds_are_the_notations_plus_github() {
        for kind in Kind::ALL {
            let expected = kind.is_notation() || *kind == Kind::Github;
            assert_eq!(
                kind.carries_questionnaire(),
                expected,
                "{} questionnaire-carrying classification",
                kind.as_str()
            );
        }
        // The load-bearing pair: github may declare one, a workshop may not.
        assert!(Kind::Github.carries_questionnaire());
        assert!(!Kind::Workshop.carries_questionnaire());
    }

    #[test]
    fn github_is_parsed_and_described() {
        assert_eq!(Kind::parse("github"), Some(Kind::Github));
        assert!(Kind::Github.describe().contains("GitHub"));
        assert!(!Kind::Github.opens_a_matter());
    }

    #[test]
    fn matter_opening_kinds_are_the_engagements_and_nothing_else() {
        // The one kind that opens a matter: the onboarding engagement,
        // whether a single-instrument engagement letter or the
        // intake-driven onboarding that opens a bundle of instruments
        // (the estate plan, fractional GC).
        assert!(
            Kind::Onboarding.opens_a_matter(),
            "onboarding opens a matter"
        );
        // Every other kind is work done *inside* an already-open matter,
        // or a content page — none of them is the engagement.
        for kind in [
            Kind::Letter,
            Kind::Filing,
            Kind::Will,
            Kind::Trust,
            Kind::Directive,
            Kind::Agreement,
            Kind::Offboarding,
            Kind::Memo,
            Kind::Event,
            Kind::Post,
            Kind::Workshop,
            Kind::Github,
            Kind::Transcript,
            Kind::InboundContract,
            Kind::CertificateOfNaturalization,
        ] {
            assert!(
                !kind.opens_a_matter(),
                "{} does not open a matter",
                kind.as_str()
            );
        }
    }

    #[test]
    fn closing_kinds_are_the_offboarding_letter_and_nothing_else() {
        // The mirror of `matter_opening_kinds_are_the_engagements_and_nothing_else`:
        // the one kind that closes a matter is the offboarding letter.
        assert!(
            Kind::Offboarding.closes_a_matter(),
            "offboarding closes a matter"
        );
        // `Kind::Letter` is the load-bearing negative case: every offboarding
        // letter is a letter, but an ordinary letter must not close a matter,
        // or any demand/notice/settlement letter would silently end the
        // representation.
        assert!(!Kind::Letter.closes_a_matter());
        // Every other kind is work done *inside* an already-open matter, the
        // engagement that opened it, or a content page — none of them is the
        // offboarding letter.
        for kind in [
            Kind::Letter,
            Kind::Filing,
            Kind::Will,
            Kind::Trust,
            Kind::Directive,
            Kind::Agreement,
            Kind::Onboarding,
            Kind::Memo,
            Kind::Event,
            Kind::Post,
            Kind::Workshop,
            Kind::Github,
            Kind::Transcript,
            Kind::InboundContract,
            Kind::CertificateOfNaturalization,
        ] {
            assert!(
                !kind.closes_a_matter(),
                "{} does not close a matter",
                kind.as_str()
            );
        }
    }

    #[test]
    fn dashboard_kinds_are_exactly_the_ten_catalog_entries() {
        let dashboards: Vec<&str> = Kind::ALL
            .iter()
            .filter(|k| k.is_dashboard())
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            dashboards,
            vec![
                "review_queue_workbench",
                "verifier_split_view",
                "matter_status_console",
                "docket_deadline_board",
                "document_workbench",
                "authority_library",
                "discovery_cockpit",
                "hearing_console",
                "deliverable_package",
                "engagement_billing_records",
            ],
        );
    }

    #[test]
    fn asset_lane_only_kinds_carry_no_notation_or_matter_semantics() {
        // None of these is notation-family, carries a questionnaire, or
        // opens a matter — they classify an `assets` row, not a template.
        // See the doc comments for the #779 Lane-scoping caveat.
        for kind in [
            Kind::Transcript,
            Kind::InboundContract,
            Kind::CertificateOfNaturalization,
        ] {
            assert!(!kind.is_notation());
            assert!(!kind.carries_questionnaire());
            assert!(!kind.opens_a_matter());
        }
        assert_eq!(Kind::parse("transcript"), Some(Kind::Transcript));
        assert_eq!(Kind::parse("inbound_contract"), Some(Kind::InboundContract));
        assert_eq!(
            Kind::parse("certificate_of_naturalization"),
            Some(Kind::CertificateOfNaturalization)
        );
    }

    #[test]
    fn declared_reads_a_recognized_kind_from_frontmatter() {
        let body = "---\ntitle: T\nkind: onboarding\n---\n\nBody.\n";
        assert_eq!(declared(body), Some(Kind::Onboarding));
    }

    #[test]
    fn declared_is_none_for_absent_or_unknown_kind() {
        assert_eq!(declared("---\ntitle: T\n---\n"), None);
        assert_eq!(declared("---\ntitle: T\nkind: bogus\n---\n"), None);
        assert_eq!(declared("no frontmatter"), None);
    }
}
