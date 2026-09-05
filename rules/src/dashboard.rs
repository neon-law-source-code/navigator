//! The matter dashboard **kind catalog** — the page types an attorney
//! composes, and the section skeleton each one is checked against (#896).
//!
//! # Composition, not code
//!
//! A dashboard kind is not a code path. Per #690, `kind:` drives
//! **validation and scaffolding, never rendering** — the renderer only
//! ever sees sections. So a kind is a named skeleton plus the rules that
//! check it, which is what makes adding one cost a rules-crate entry
//! rather than a component.
//!
//! Each entry in [`Kind::is_dashboard`]'s catalog recurred across
//! independently-authored matter hubs in the surveyed corpus. What
//! transfers from that survey is the **product knowledge** — which
//! dashboards lawyers build when nobody constrains them, and what each one
//! contains. None of the implementation transfers.
//!
//! # An authored composition
//!
//! A composition is Markdown with YAML frontmatter, the same authoring
//! seam as a notation template:
//!
//! ```yaml
//! kind: review_queue_workbench
//! title: Document review — Homer v. Flanders
//! lenses:
//!   lawyer: [queue_rail, item_detail, item_status_setter, boundary_note, provenance_statement]
//!   client: [boundary_note, provenance_statement]
//! ```
//!
//! `lenses:` is the per-lens composition #690 requires: the client, lawyer,
//! and clerk section lists live in **one file**, so the faces of a
//! dashboard cannot drift apart across separate documents.
//!
//! # What every kind carries
//!
//! Two sections are in every skeleton regardless of kind, because the
//! survey found them in 13 of 13 surfaces in one batch and 8 of 9 in
//! another:
//!
//! - [`Section::BoundaryNote`] — what this page is *not*, and what still
//!   requires a human.
//! - [`Section::ProvenanceStatement`] — as-of date, what was examined,
//!   what was not.
//!
//! They are part of the skeleton rather than left to the author, and
//! [`D003RequiredSection`](crate::D003RequiredSection) holds **every**
//! declared lens to both of them. A client face without a boundary note is
//! the failure this rule exists to prevent.
//!
//! # Boundaries with the neighbouring issues
//!
//! - **#888** owns the component registry. Each [`Section`] maps 1:1 to a
//!   component there; this enum is the authoring half of that pairing and
//!   grows when #888 adds a section type.
//! - **#863** owns the visibility boundary. This module says which
//!   sections a *kind* may carry, never which sections a *client* may see
//!   — that is authorization, and it does not belong in a lint rule.
//! - **#895** consumes [`scaffold`] and [`catalog`]; **#890** owns the
//!   authority and citation-disposition vocabulary the authority library
//!   and verifier render.

use std::fmt::Write as _;

use crate::kind::Kind;

/// The two families the surveyed corpus splits into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// An interactive surface a case team works in and iterates on.
    Dashboard,
    /// A linear, non-interactive handoff page. It is read, not worked in.
    Deliverable,
}

/// A face of a dashboard. The three lists are authored in one file, so a
/// kind's faces cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lens {
    /// What a client participant sees.
    Client,
    /// What the firm sees.
    Lawyer,
    /// What a court clerk or comparable outside reader sees.
    Clerk,
}

impl Lens {
    /// Every lens, in declaration order.
    pub const ALL: &'static [Lens] = &[Lens::Client, Lens::Lawyer, Lens::Clerk];

    /// The key this lens is authored under in `lenses:`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Lens::Client => "client",
            Lens::Lawyer => "lawyer",
            Lens::Clerk => "clerk",
        }
    }

    /// Parse a `lenses:` key, or `None` when it names no known lens.
    #[must_use]
    pub fn parse(value: &str) -> Option<Lens> {
        Self::ALL.iter().copied().find(|l| l.as_str() == value)
    }
}

/// A section type an authored composition may declare.
///
/// Closed on purpose: an author picks from a kind's catalog, and anything
/// outside it is a validation error rather than a silently-dropped
/// section. A shape the registry lacks is a pull request against #888 that
/// then benefits every matter and every tenant — never a hand-written
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    /// What this page is not, and what still requires a human. Universal.
    BoundaryNote,
    /// As-of date, what was examined, what was not. Universal.
    ProvenanceStatement,
    /// A filterable rail of the items under review.
    QueueRail,
    /// The detail pane for the selected item.
    ItemDetail,
    /// A per-item status and note the reviewer sets.
    ItemStatusSetter,
    /// How far through the queue the reviewer is.
    ProgressCounter,
    /// Prev/next movement through an ordered set.
    Stepper,
    /// The drafted assertion under verification.
    AssertionPanel,
    /// The source page region the assertion rests on (#893).
    SourcePagePanel,
    /// The headline numbers for a matter at a glance.
    MetricStrip,
    /// One card per workstream, each with a status chip.
    WorkstreamCards,
    /// The dates that matter, in order.
    KeyDatesStrip,
    /// Decisions only the principal can make.
    DecisionQueue,
    /// Dated rows with hard/soft classification, provenance confidence,
    /// and per-row calendar export (#539, #688).
    DeadlineBoard,
    /// A draft's block-structured Markdown body. Pagination and paper
    /// geometry are Typst's job (#889), never the browser's.
    DocumentBody,
    /// Phrase-anchored comments alongside a document body.
    CommentRail,
    /// Authorities with citation, holding, verbatim quote, and position
    /// for this matter (#890).
    AuthorityTable,
    /// Numbered request/response pairs with objection tracking.
    DiscoveryPairs,
    /// Ordered stages with argument points, authorities, and anticipated
    /// questions.
    StageList,
    /// What a package includes and, as importantly, what it excludes.
    PackageManifest,
    /// The files a reader takes away.
    DownloadList,
    /// The engagement agreement as a record.
    EngagementLetter,
    /// An issued invoice as a record.
    Invoice,
    /// Prebill review with per-entry findings.
    PrebillReview,
}

/// The sections every kind carries, whatever else it carries.
pub const UNIVERSAL: &[Section] = &[Section::BoundaryNote, Section::ProvenanceStatement];

impl Section {
    /// Every section type, in declaration order.
    pub const ALL: &'static [Section] = &[
        Section::BoundaryNote,
        Section::ProvenanceStatement,
        Section::QueueRail,
        Section::ItemDetail,
        Section::ItemStatusSetter,
        Section::ProgressCounter,
        Section::Stepper,
        Section::AssertionPanel,
        Section::SourcePagePanel,
        Section::MetricStrip,
        Section::WorkstreamCards,
        Section::KeyDatesStrip,
        Section::DecisionQueue,
        Section::DeadlineBoard,
        Section::DocumentBody,
        Section::CommentRail,
        Section::AuthorityTable,
        Section::DiscoveryPairs,
        Section::StageList,
        Section::PackageManifest,
        Section::DownloadList,
        Section::EngagementLetter,
        Section::Invoice,
        Section::PrebillReview,
    ];

    /// The name this section is authored as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Section::BoundaryNote => "boundary_note",
            Section::ProvenanceStatement => "provenance_statement",
            Section::QueueRail => "queue_rail",
            Section::ItemDetail => "item_detail",
            Section::ItemStatusSetter => "item_status_setter",
            Section::ProgressCounter => "progress_counter",
            Section::Stepper => "stepper",
            Section::AssertionPanel => "assertion_panel",
            Section::SourcePagePanel => "source_page_panel",
            Section::MetricStrip => "metric_strip",
            Section::WorkstreamCards => "workstream_cards",
            Section::KeyDatesStrip => "key_dates_strip",
            Section::DecisionQueue => "decision_queue",
            Section::DeadlineBoard => "deadline_board",
            Section::DocumentBody => "document_body",
            Section::CommentRail => "comment_rail",
            Section::AuthorityTable => "authority_table",
            Section::DiscoveryPairs => "discovery_pairs",
            Section::StageList => "stage_list",
            Section::PackageManifest => "package_manifest",
            Section::DownloadList => "download_list",
            Section::EngagementLetter => "engagement_letter",
            Section::Invoice => "invoice",
            Section::PrebillReview => "prebill_review",
        }
    }

    /// A one-line summary — the surface an editor completion shows.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Section::BoundaryNote => "What this page is not, and what still requires a human",
            Section::ProvenanceStatement => {
                "As-of date, what was examined, and what was not examined"
            }
            Section::QueueRail => "A filterable rail of the items under review",
            Section::ItemDetail => "The detail pane for the selected item",
            Section::ItemStatusSetter => "A per-item status and note the reviewer sets",
            Section::ProgressCounter => "How far through the queue the reviewer is",
            Section::Stepper => "Prev/next movement through an ordered set",
            Section::AssertionPanel => "The drafted assertion under verification",
            Section::SourcePagePanel => "The source page region the assertion rests on",
            Section::MetricStrip => "The headline numbers for the matter at a glance",
            Section::WorkstreamCards => "One card per workstream, each with a status chip",
            Section::KeyDatesStrip => "The dates that matter, in order",
            Section::DecisionQueue => "Decisions only the principal can make",
            Section::DeadlineBoard => {
                "Dated rows with hard/soft classification and calendar export"
            }
            Section::DocumentBody => "A draft's block-structured Markdown body",
            Section::CommentRail => "Phrase-anchored comments alongside a document body",
            Section::AuthorityTable => "Authorities with citation, holding, quote, and position",
            Section::DiscoveryPairs => "Numbered request/response pairs with objection tracking",
            Section::StageList => "Ordered stages with argument points and anticipated questions",
            Section::PackageManifest => "What the package includes, and what it excludes",
            Section::DownloadList => "The files a reader takes away",
            Section::EngagementLetter => "The engagement agreement as a record",
            Section::Invoice => "An issued invoice as a record",
            Section::PrebillReview => "Prebill review with per-entry findings",
        }
    }

    /// Parse an authored section name, or `None` when it names nothing in
    /// the vocabulary.
    #[must_use]
    pub fn parse(value: &str) -> Option<Section> {
        Self::ALL.iter().copied().find(|s| s.as_str() == value)
    }

    /// The section's name as a prose heading — what [`scaffold`] writes
    /// above the stub. Derived from [`Section::as_str`] so the two can
    /// never drift.
    #[must_use]
    pub fn heading(self) -> String {
        let spaced = self.as_str().replace('_', " ");
        let mut chars = spaced.chars();
        chars.next().map_or(spaced.clone(), |first| {
            first.to_uppercase().collect::<String>() + chars.as_str()
        })
    }
}

/// One kind's skeleton: what it must carry to be that kind, and what else
/// it may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skeleton {
    /// Which family the kind belongs to.
    pub family: Family,
    /// The spine. Without these the page is not this kind, so each must
    /// appear in at least one declared lens.
    pub required: &'static [Section],
    /// Sections the kind may carry. Not required anywhere.
    pub optional: &'static [Section],
}

impl Skeleton {
    /// Every section this kind may declare: its spine, its options, and
    /// the universal two. The allowlist
    /// [`D002OutOfCatalogSection`](crate::D002OutOfCatalogSection) checks
    /// against.
    #[must_use]
    pub fn catalog(&self) -> Vec<Section> {
        let mut out: Vec<Section> = UNIVERSAL.to_vec();
        out.extend_from_slice(self.required);
        out.extend_from_slice(self.optional);
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The skeleton for a dashboard kind, or `None` for any other [`Kind`].
///
/// Exhaustive over [`Kind`] on purpose: adding a dashboard kind fails to
/// compile until it declares a skeleton, so a kind can never reach an
/// author with no catalog behind it.
#[must_use]
pub fn skeleton(kind: Kind) -> Option<Skeleton> {
    use Section as S;
    let s = match kind {
        // The dominant layout in the corpus — six of nine surfaces in one
        // batch. Document review, discovery response, and citation
        // checking are all specialisations of it.
        Kind::ReviewQueueWorkbench => Skeleton {
            family: Family::Dashboard,
            required: &[S::QueueRail, S::ItemDetail, S::ItemStatusSetter],
            optional: &[S::ProgressCounter, S::Stepper],
        },
        // Three hubs built this independently: drafted assertion on the
        // left, source page on the right, both marked (#891, #893).
        Kind::VerifierSplitView => Skeleton {
            family: Family::Dashboard,
            required: &[S::AssertionPanel, S::SourcePagePanel, S::ItemStatusSetter],
            optional: &[S::Stepper, S::ProgressCounter, S::AuthorityTable],
        },
        Kind::MatterStatusConsole => Skeleton {
            family: Family::Dashboard,
            required: &[S::MetricStrip, S::WorkstreamCards, S::DecisionQueue],
            optional: &[S::KeyDatesStrip],
        },
        Kind::DocketDeadlineBoard => Skeleton {
            family: Family::Dashboard,
            required: &[S::DeadlineBoard],
            optional: &[S::KeyDatesStrip, S::MetricStrip],
        },
        Kind::DocumentWorkbench => Skeleton {
            family: Family::Dashboard,
            required: &[S::DocumentBody],
            optional: &[S::CommentRail, S::DownloadList],
        },
        Kind::AuthorityLibrary => Skeleton {
            family: Family::Dashboard,
            required: &[S::AuthorityTable],
            optional: &[S::SourcePagePanel],
        },
        Kind::DiscoveryCockpit => Skeleton {
            family: Family::Dashboard,
            required: &[S::DiscoveryPairs],
            optional: &[S::ProgressCounter, S::ItemStatusSetter, S::DownloadList],
        },
        Kind::HearingConsole => Skeleton {
            family: Family::Dashboard,
            required: &[S::StageList],
            optional: &[S::AuthorityTable, S::KeyDatesStrip, S::Stepper],
        },
        // The second family: linear, non-interactive, read rather than
        // worked in.
        Kind::DeliverablePackage => Skeleton {
            family: Family::Deliverable,
            required: &[S::PackageManifest],
            optional: &[S::DownloadList],
        },
        // A Dashboard rather than a Deliverable because prebill review is
        // a surface the firm works in; the letter and invoice beside it
        // are the records that review is against.
        Kind::EngagementBillingRecords => Skeleton {
            family: Family::Dashboard,
            required: &[S::EngagementLetter, S::Invoice],
            optional: &[S::PrebillReview, S::DownloadList],
        },
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
        | Kind::Transcript
        | Kind::InboundContract
        | Kind::CertificateOfNaturalization
        | Kind::Exhibit
        | Kind::Unclassified => return None,
    };
    Some(s)
}

/// Every section a dashboard kind may declare, or an empty vector for a
/// non-dashboard kind. The surface #895's scaffolder and the LSP's
/// completion both read.
#[must_use]
pub fn catalog(kind: Kind) -> Vec<Section> {
    skeleton(kind).map(|s| s.catalog()).unwrap_or_default()
}

/// Scaffold an authored composition for `kind`, titled `title`.
///
/// The lawyer lens gets the kind's spine plus the universal two; the client
/// lens gets only the universal two. That default is deliberate — a client
/// face starts with nothing firm-side on it and the attorney adds what the
/// client should see, rather than starting from everything and
/// remembering to remove.
///
/// Returns `None` for a kind that is not a dashboard kind.
#[must_use]
pub fn scaffold(kind: Kind, title: &str) -> Option<String> {
    let skeleton = skeleton(kind)?;
    let lawyer: Vec<&str> = skeleton
        .required
        .iter()
        .chain(UNIVERSAL.iter())
        .map(|s| s.as_str())
        .collect();
    let client: Vec<&str> = UNIVERSAL.iter().map(|s| s.as_str()).collect();

    let mut out = String::new();
    // Writing to a `String` is infallible, so the results are discarded
    // rather than propagated — the same idiom the rest of the workspace
    // uses for `write!` into an in-memory buffer.
    let _ = write!(
        &mut out,
        "---\nkind: {}\ntitle: {title}\nlenses:\n  lawyer: [{}]\n  client: [{}]\n---\n\n# {title}\n",
        kind.as_str(),
        lawyer.join(", "),
        client.join(", "),
    );
    for section in skeleton.required.iter().chain(UNIVERSAL.iter()) {
        let _ = write!(
            &mut out,
            "\n## {}\n\n{}.\n",
            section.heading(),
            section.describe(),
        );
    }
    Some(out)
}

/// The `lenses:` block of an authored composition: each declared lens and
/// the section names under it, exactly as authored.
///
/// Section names are returned as raw strings rather than [`Section`]s so
/// [`D001UnknownSection`](crate::D001UnknownSection) can report the
/// offending text. Returns `None` when the file declares no `lenses:` key
/// or the frontmatter does not parse.
#[must_use]
pub fn declared_lenses(contents: &str) -> Option<Vec<(String, Vec<String>)>> {
    let fm = crate::frontmatter::extract(contents)?;
    let value: serde_yaml::Value = serde_yaml::from_str(fm).ok()?;
    let mapping = value.as_mapping()?.get("lenses")?.as_mapping()?;
    let mut out = Vec::new();
    for (key, list) in mapping {
        let Some(name) = key.as_str() else {
            continue;
        };
        let sections = list
            .as_sequence()
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        out.push((name.to_string(), sections));
    }
    Some(out)
}

/// Every section name, for a diagnostic that has to spell the vocabulary
/// out.
#[must_use]
pub fn section_names() -> Vec<&'static str> {
    Section::ALL.iter().map(|s| s.as_str()).collect()
}

/// The 1-based line of the top-level `lenses:` key, so a diagnostic about
/// the composition as a whole underlines the block rather than the file's
/// first line. Falls back to line 1.
#[must_use]
pub fn lenses_line(contents: &str) -> usize {
    contents
        .lines()
        .enumerate()
        .take_while(|(idx, line)| *idx == 0 || *line != "---")
        .find(|(_, line)| !line.starts_with([' ', '\t']) && line.starts_with("lenses:"))
        .map_or(1, |(idx, _)| idx + 1)
}

/// The 1-based line inside the frontmatter that mentions `name`, so a
/// per-section diagnostic underlines where the author wrote it. Falls back
/// to the `lenses:` line when the name cannot be located — a section
/// written in flow style shares its line with its neighbours, which is
/// exactly where the author is looking.
#[must_use]
pub fn section_line(contents: &str, name: &str) -> usize {
    contents
        .lines()
        .enumerate()
        .take_while(|(idx, line)| *idx == 0 || *line != "---")
        .find(|(_, line)| line.contains(name))
        .map_or_else(|| lenses_line(contents), |(idx, _)| idx + 1)
}

#[cfg(test)]
mod tests {
    use super::{
        catalog, declared_lenses, lenses_line, scaffold, section_line, skeleton, Family, Lens,
        Section, UNIVERSAL,
    };
    use crate::kind::Kind;

    /// Every dashboard kind, taken from the enum rather than re-listed, so
    /// a new kind joins these tests automatically.
    fn dashboard_kinds() -> Vec<Kind> {
        Kind::ALL
            .iter()
            .copied()
            .filter(|k| k.is_dashboard())
            .collect()
    }

    #[test]
    fn every_dashboard_kind_has_a_skeleton_and_no_other_kind_does() {
        for kind in Kind::ALL {
            assert_eq!(
                skeleton(*kind).is_some(),
                kind.is_dashboard(),
                "{} skeleton/is_dashboard disagree",
                kind.as_str(),
            );
        }
        assert_eq!(dashboard_kinds().len(), 10, "the catalog is ten kinds");
    }

    #[test]
    fn every_skeleton_has_a_spine() {
        // A kind with no required section is not a skeleton, it is a
        // blank file with a name.
        for kind in dashboard_kinds() {
            let s = skeleton(kind).unwrap();
            assert!(
                !s.required.is_empty(),
                "{} declares no required section",
                kind.as_str(),
            );
        }
    }

    #[test]
    fn the_universal_sections_are_in_every_catalog() {
        for kind in dashboard_kinds() {
            let catalog = catalog(kind);
            for section in UNIVERSAL {
                assert!(
                    catalog.contains(section),
                    "{} catalog is missing {}",
                    kind.as_str(),
                    section.as_str(),
                );
            }
        }
    }

    #[test]
    fn a_catalog_never_repeats_a_section() {
        // `required` and `optional` are hand-written; a section listed in
        // both would make the allowlist ambiguous.
        for kind in dashboard_kinds() {
            let s = skeleton(kind).unwrap();
            for section in s.required {
                assert!(
                    !s.optional.contains(section),
                    "{} lists {} as both required and optional",
                    kind.as_str(),
                    section.as_str(),
                );
                assert!(
                    !UNIVERSAL.contains(section),
                    "{} re-declares the universal section {}",
                    kind.as_str(),
                    section.as_str(),
                );
            }
        }
    }

    #[test]
    fn the_corpus_splits_into_two_families() {
        assert_eq!(
            skeleton(Kind::DeliverablePackage).unwrap().family,
            Family::Deliverable,
        );
        assert_eq!(
            skeleton(Kind::ReviewQueueWorkbench).unwrap().family,
            Family::Dashboard,
        );
    }

    #[test]
    fn section_round_trips_and_rejects_unknown_names() {
        for section in Section::ALL {
            assert_eq!(Section::parse(section.as_str()), Some(*section));
            assert!(!section.describe().is_empty());
        }
        assert_eq!(Section::parse("timeline_of_vibes"), None);
        assert_eq!(Section::parse(""), None);
    }

    #[test]
    fn section_names_are_unique() {
        let mut names: Vec<&str> = Section::ALL.iter().map(|s| s.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two sections share a name");
    }

    #[test]
    fn lens_round_trips_and_rejects_unknown_names() {
        for lens in Lens::ALL {
            assert_eq!(Lens::parse(lens.as_str()), Some(*lens));
        }
        assert_eq!(Lens::parse("partner"), None);
    }

    #[test]
    fn a_scaffold_declares_the_spine_on_lawyer_and_only_the_universal_on_client() {
        let out = scaffold(Kind::MatterStatusConsole, "Homer v. Flanders").unwrap();
        let lenses = declared_lenses(&out).unwrap();
        let lawyer = &lenses.iter().find(|(n, _)| n == "lawyer").unwrap().1;
        let client = &lenses.iter().find(|(n, _)| n == "client").unwrap().1;

        assert!(lawyer.contains(&"metric_strip".to_string()));
        assert!(lawyer.contains(&"decision_queue".to_string()));
        // The client face starts with nothing firm-side on it. Here that
        // matters: a decision queue is the firm's own reasoning.
        assert!(!client.contains(&"decision_queue".to_string()));
        assert_eq!(client, &vec!["boundary_note", "provenance_statement"]);
    }

    #[test]
    fn scaffold_returns_none_for_a_non_dashboard_kind() {
        assert!(scaffold(Kind::Onboarding, "Nope").is_none());
        assert!(catalog(Kind::Onboarding).is_empty());
    }

    #[test]
    fn declared_lenses_reads_both_yaml_sequence_forms() {
        let flow = "---\nkind: authority_library\nlenses:\n  lawyer: [authority_table]\n---\n";
        let block =
            "---\nkind: authority_library\nlenses:\n  lawyer:\n    - authority_table\n---\n";
        for body in [flow, block] {
            assert_eq!(
                declared_lenses(body).unwrap(),
                vec![("lawyer".to_string(), vec!["authority_table".to_string()])],
            );
        }
    }

    #[test]
    fn diagnostics_point_at_the_line_the_author_wrote() {
        let body = "---\nkind: authority_library\ntitle: T\nlenses:\n  lawyer:\n    - authority_table\n---\n";
        assert_eq!(lenses_line(body), 4);
        assert_eq!(section_line(body, "authority_table"), 6);
        // An unlocatable name falls back to the block, never to line 1
        // when a block exists.
        assert_eq!(section_line(body, "not_written_anywhere"), 4);
    }

    #[test]
    fn declared_lenses_is_none_without_a_lenses_block() {
        assert!(declared_lenses("---\nkind: authority_library\n---\n").is_none());
        assert!(declared_lenses("no frontmatter at all").is_none());
    }
}
