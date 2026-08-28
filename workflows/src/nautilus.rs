//! Inbound-triage classification and statutory-deadline calculation for
//! the Neon Law Nautilus consumer-report screening shield.
//!
//! Workflow 02 turns an inbound message on an active Nautilus matter into
//! the right downstream action. The classifier here is pure logic — it
//! reads the subject and body and returns a [`ScreeningMailClass`];
//! [`route`] maps that class to the sub-workflow that handles it. The live
//! `workflows-service` worker calls these over each inbound `.eml` that
//! threads onto an active Nautilus matter — an adverse-action notice, a
//! forwarded screening report, or a consumer reporting agency's
//! reinvestigation result.
//!
//! Two rules are load-bearing and grounded in statute:
//!
//! 1. **Explicit live litigation is detected first and referred out.** A
//!    message saying the client is being sued, was served, or received an
//!    enclosed summons is classified ahead of every other category so a
//!    dispute phrase buried in a court document can never mask it. Historical
//!    court-record terms inside a screening report stay in the dispute path.
//!    A live lawsuit is never answered as correspondence — it goes to
//!    litigation counsel.
//! 2. **The statutory windows are calendared from one calculator.**
//!    [`DeadlineKind`] carries the FCRA §1681i(a)(1) 30-day
//!    reinvestigation period and the §1681j(b) 60-day post-adverse-action
//!    free-report window, so the workflows calendar them from one place
//!    rather than hard-coding a number twice.

use chrono::{Duration, NaiveDate};

/// Classification of an inbound message against an active Nautilus matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreeningMailClass {
    /// A summons, complaint, or lawsuit notice. Refer to litigation
    /// counsel — never answered as correspondence.
    LawsuitOrSummons,
    /// A consumer reporting agency's response to a §1681i dispute — its
    /// reinvestigation result. Queued for attorney review (workflow 03).
    ReinvestigationResult,
    /// A landlord's or employer's adverse-action notice: an application
    /// denied based on a consumer report. Opens the §1681i dispute and the
    /// §1681j(b) 60-day free-report window (workflow 01).
    AdverseAction,
    /// The consumer's own screening or background report, forwarded to
    /// dispute. Opens the §1681i dispute (workflow 01).
    ReportForwarded,
    /// Anything we cannot confidently route — flag for a lawyer.
    Other,
}

/// Phrases that mark a court action. Checked first; a match here wins over
/// every other category.
const LAWSUIT_MARKERS: &[&str] = &[
    "summons",
    "complaint",
    "you are being sued",
    "being sued",
    "notice of lawsuit",
    "civil action",
    "unlawful detainer",
    "eviction lawsuit",
    "eviction summons",
    "writ of",
    "garnish",
    "judgment",
    "served with",
];

/// Phrases that specifically mark a live lawsuit or summons, rather than a
/// historical court-record item described inside a screening report.
const LIVE_LAWSUIT_MARKERS: &[&str] = &[
    "you are being sued",
    "being sued",
    "notice of lawsuit",
    "civil action",
    "summons is enclosed",
    "summons enclosed",
    "complaint is enclosed",
    "complaint enclosed",
    "served with",
    "writ of",
    "garnish",
];

/// Phrases that mark a consumer reporting agency's reinvestigation result.
const REINVESTIGATION_MARKERS: &[&str] = &[
    "results of your reinvestigation",
    "results of our reinvestigation",
    "reinvestigation is complete",
    "completed our investigation",
    "your dispute results",
    "results of your dispute",
    "dispute has been processed",
    "outcome of your dispute",
];

/// Phrases that mark a landlord's or employer's adverse-action notice.
const ADVERSE_ACTION_MARKERS: &[&str] = &[
    "adverse action",
    "based on information in your consumer report",
    "based on your consumer report",
    "your application was denied",
    "denied your application",
    "unable to approve your application",
    "did not meet our screening criteria",
    "rental application",
];

/// Phrases that mark the consumer's own screening or background report,
/// forwarded to dispute.
const REPORT_MARKERS: &[&str] = &[
    "tenant screening",
    "screening report",
    "background check",
    "background report",
    "consumer report",
    "report attached",
    "criminal record",
    "eviction record",
    "rental history report",
];

/// Classify an inbound message from its subject and body. The precedence is
/// intentional (explicit live lawsuit → reinvestigation result → adverse
/// action → forwarded report → lawsuit marker → other); see the module docs.
#[must_use]
pub fn classify(subject: &str, body: &str) -> ScreeningMailClass {
    let subject = subject.to_lowercase();
    let hay = format!("{subject} {}", body.to_lowercase());
    let has = |needles: &[&str]| needles.iter().any(|n| hay.contains(n));
    if has(LIVE_LAWSUIT_MARKERS) {
        ScreeningMailClass::LawsuitOrSummons
    } else if has(REINVESTIGATION_MARKERS) {
        ScreeningMailClass::ReinvestigationResult
    } else if has(ADVERSE_ACTION_MARKERS) {
        ScreeningMailClass::AdverseAction
    } else if has(REPORT_MARKERS) {
        ScreeningMailClass::ReportForwarded
    } else if has(LAWSUIT_MARKERS) {
        ScreeningMailClass::LawsuitOrSummons
    } else {
        ScreeningMailClass::Other
    }
}

/// Where a classified inbound message is routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageRoute {
    /// Refer to litigation counsel (Sethi Legal). A lawsuit or summons is
    /// never answered as correspondence.
    ReferLitigation,
    /// Invoke the consumer-report dispute workflow (01): draft the §1681i
    /// dispute letter for attorney review.
    OpenDispute,
    /// Route the agency's reinvestigation result to attorney review (03).
    ReinvestigationReview,
    /// No active matter matched, or an unroutable message — flag for a
    /// lawyer.
    LawyerReview,
}

/// Map a classification to the sub-workflow that handles it. An
/// adverse-action notice and a forwarded report both open the dispute
/// workflow (01): the denial opens the matter and the 60-day free-report
/// window, the forwarded report is the item the dispute acts on.
#[must_use]
pub fn route(class: ScreeningMailClass) -> TriageRoute {
    match class {
        ScreeningMailClass::LawsuitOrSummons => TriageRoute::ReferLitigation,
        ScreeningMailClass::AdverseAction | ScreeningMailClass::ReportForwarded => {
            TriageRoute::OpenDispute
        }
        ScreeningMailClass::ReinvestigationResult => TriageRoute::ReinvestigationReview,
        ScreeningMailClass::Other => TriageRoute::LawyerReview,
    }
}

/// A statutory deadline opened by a routed Nautilus message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatutoryDeadline {
    /// The statutory window being tracked.
    pub kind: DeadlineKind,
    /// The date the window starts.
    pub trigger_on: NaiveDate,
    /// The date the window closes.
    pub due_on: NaiveDate,
    /// The official citation shown to lawyer and clients.
    pub statute: &'static str,
}

impl StatutoryDeadline {
    /// Build the durable deadline record for a statutory window.
    #[must_use]
    pub fn new(kind: DeadlineKind, trigger_on: NaiveDate) -> Self {
        Self {
            kind,
            trigger_on,
            due_on: deadline_from(kind, trigger_on),
            statute: kind.statute(),
        }
    }

    /// Stable storage token for the durable deadline row.
    #[must_use]
    pub const fn storage_kind(self) -> &'static str {
        self.kind.storage_token()
    }
}

/// The routed result of inbound Nautilus triage, including any statutory
/// deadlines the workflow must persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageDecision {
    /// The content classification.
    pub class: ScreeningMailClass,
    /// The workflow branch that handles the message.
    pub route: TriageRoute,
    /// Statutory windows opened by the message and ready for persistence by
    /// the deadline spine.
    pub deadlines: Vec<StatutoryDeadline>,
}

/// Statutory windows opened by a classified message on an active matter.
#[must_use]
pub fn deadlines_for(class: ScreeningMailClass, trigger_on: NaiveDate) -> Vec<StatutoryDeadline> {
    match class {
        ScreeningMailClass::AdverseAction => vec![
            StatutoryDeadline::new(DeadlineKind::FcraReinvestigation, trigger_on),
            StatutoryDeadline::new(DeadlineKind::AdverseActionFreeReport, trigger_on),
        ],
        ScreeningMailClass::ReportForwarded => {
            vec![StatutoryDeadline::new(
                DeadlineKind::FcraReinvestigation,
                trigger_on,
            )]
        }
        ScreeningMailClass::LawsuitOrSummons
        | ScreeningMailClass::ReinvestigationResult
        | ScreeningMailClass::Other => Vec::new(),
    }
}

/// Triage an inbound message end to end. A message whose sender does not
/// match an active Nautilus matter is always flagged for lawyer, whatever
/// its content — we never auto-route mail we can't tie to a represented
/// client.
#[must_use]
pub fn triage(
    has_active_matter: bool,
    subject: &str,
    body: &str,
    received_on: NaiveDate,
) -> TriageDecision {
    let class = classify(subject, body);
    let route = if has_active_matter {
        route(class)
    } else {
        TriageRoute::LawyerReview
    };
    let deadlines = if has_active_matter {
        deadlines_for(class, received_on)
    } else {
        Vec::new()
    };
    TriageDecision {
        class,
        route,
        deadlines,
    }
}

/// A statutory deadline the deadline spine tracks as a durable timer and
/// surfaces in the client portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineKind {
    /// FCRA §1681i(a)(1): the consumer reporting agency's 30-day
    /// reinvestigation period, running from receipt of the dispute.
    FcraReinvestigation,
    /// FCRA §1681j(b): the consumer's 60-day window to request a free
    /// report from the agency named in an adverse-action notice, running
    /// from receipt of that notice.
    AdverseActionFreeReport,
}

impl DeadlineKind {
    /// The statutory length of the window, in days.
    #[must_use]
    pub const fn days(self) -> i64 {
        match self {
            DeadlineKind::FcraReinvestigation => 30,
            DeadlineKind::AdverseActionFreeReport => 60,
        }
    }

    /// The official citation for the window, for the portal and the
    /// journal.
    #[must_use]
    pub const fn statute(self) -> &'static str {
        match self {
            DeadlineKind::FcraReinvestigation => "15 U.S.C. § 1681i(a)(1)",
            DeadlineKind::AdverseActionFreeReport => "15 U.S.C. § 1681j(b)",
        }
    }

    /// Stable database token for this deadline kind.
    #[must_use]
    pub const fn storage_token(self) -> &'static str {
        match self {
            DeadlineKind::FcraReinvestigation => "fcra_reinvestigation",
            DeadlineKind::AdverseActionFreeReport => "adverse_action_free_report",
        }
    }
}

/// The date a statutory window closes, given the date it was triggered.
#[must_use]
pub fn deadline_from(kind: DeadlineKind, trigger: NaiveDate) -> NaiveDate {
    trigger + Duration::days(kind.days())
}

/// The result of a consumer reporting agency's FCRA §1681i reinvestigation
/// of a disputed item, surfaced to the client in plain language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FcraDisputeResult {
    /// The agency corrected or deleted the disputed item — the
    /// client-favorable outcome.
    CorrectedOrDeleted,
    /// The agency verified the item as accurate and left it unchanged.
    VerifiedUnchanged,
}

/// Phrases that mark a corrected or deleted item.
const FCRA_FIXED_MARKERS: &[&str] = &["deleted", "removed", "corrected", "updated", "modified"];

/// Classify a consumer reporting agency's reinvestigation response. A
/// correction or deletion wins; otherwise the item is treated as
/// verified-unchanged, so an ambiguous response is never reported to the
/// client as fixed.
#[must_use]
pub fn classify_fcra_result(body: &str) -> FcraDisputeResult {
    let hay = body.to_lowercase();
    if FCRA_FIXED_MARKERS.iter().any(|n| hay.contains(n)) {
        FcraDisputeResult::CorrectedOrDeleted
    } else {
        FcraDisputeResult::VerifiedUnchanged
    }
}

/// A litigation referral. Nautilus halts and hands the matter to
/// litigation counsel rather than answering the correspondence. A lawsuit,
/// a summons, or a viable FCRA damages claim leaves the dispute shield the
/// moment it appears — this is the boundary that keeps Nautilus inside the
/// firm's no-litigation identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LitigationReferral {
    /// Why the matter is being referred (e.g. "a summons was served").
    pub reason: String,
    /// The site route to reach litigation counsel — the firm's contact
    /// page, where every engagement is priced.
    pub counsel_link: &'static str,
    /// Always false: a referred matter is never answered as
    /// correspondence.
    pub answered_as_correspondence: bool,
}

/// Build the litigation referral for a matter that has left the dispute
/// shield.
#[must_use]
pub fn litigation_referral(reason: impl Into<String>) -> LitigationReferral {
    LitigationReferral {
        reason: reason.into(),
        counsel_link: "mailto:contact@neonlaw.com",
        answered_as_correspondence: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summons_is_classified_as_litigation_even_with_a_dispute_word() {
        // Precedence: a dispute phrase buried in a summons must not mask
        // the lawsuit — litigation is detected first.
        let class = classify(
            "SUMMONS — Unlawful Detainer",
            "You are being sued. You may dispute the consumer report cited in this action.",
        );
        assert_eq!(class, ScreeningMailClass::LawsuitOrSummons);
        assert_eq!(route(class), TriageRoute::ReferLitigation);
    }

    #[test]
    fn an_eviction_lawsuit_is_still_litigation() {
        let class = classify(
            "Eviction lawsuit",
            "You are being sued and an eviction summons is enclosed.",
        );
        assert_eq!(class, ScreeningMailClass::LawsuitOrSummons);
        assert_eq!(route(class), TriageRoute::ReferLitigation);
    }

    #[test]
    fn a_reinvestigation_result_routes_to_review() {
        let class = classify(
            "Re: your dispute",
            "The results of your reinvestigation are enclosed; one item was updated.",
        );
        assert_eq!(class, ScreeningMailClass::ReinvestigationResult);
        assert_eq!(route(class), TriageRoute::ReinvestigationReview);
    }

    #[test]
    fn an_adverse_action_notice_opens_a_dispute() {
        let class = classify(
            "Your rental application",
            "We denied your application based on information in your consumer report.",
        );
        assert_eq!(class, ScreeningMailClass::AdverseAction);
        assert_eq!(route(class), TriageRoute::OpenDispute);
    }

    #[test]
    fn a_forwarded_report_opens_a_dispute() {
        let class = classify(
            "My screening report",
            "Attached is the tenant screening report the landlord ran; the eviction is not mine.",
        );
        assert_eq!(class, ScreeningMailClass::ReportForwarded);
        assert_eq!(route(class), TriageRoute::OpenDispute);
    }

    #[test]
    fn historical_eviction_record_language_inside_a_report_stays_a_dispute() {
        for (subject, body) in [
            (
                "My screening report",
                "Attached is my screening report; the eviction lawsuit is not mine.",
            ),
            (
                "My screening report",
                "Attached is my screening report; the eviction summons belongs to another consumer.",
            ),
            (
                "My screening report",
                "Attached is my screening report; the unlawful detainer judgment is inaccurate.",
            ),
            (
                "Fwd: unlawful detainer on my screening report",
                "This forwarded report lists an eviction record that belongs to someone else.",
            ),
            (
                "Lawsuit eviction not mine — report attached",
                "The tenant screening company put this court record on my background report.",
            ),
        ] {
            let class = classify(subject, body);
            assert_eq!(class, ScreeningMailClass::ReportForwarded, "{subject}");
            assert_eq!(route(class), TriageRoute::OpenDispute, "{subject}");
        }
    }

    #[test]
    fn an_unrecognized_message_is_flagged_for_lawyer() {
        let class = classify("Hello", "Please call our office at your convenience.");
        assert_eq!(class, ScreeningMailClass::Other);
        assert_eq!(route(class), TriageRoute::LawyerReview);
    }

    #[test]
    fn an_unmatched_sender_is_always_flagged_for_lawyer() {
        // Even a routine adverse-action notice is lawyer-flagged when we
        // can't tie the sender to a represented client.
        let decision = triage(
            false,
            "Your rental application",
            "We denied your application based on your consumer report.",
            NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
        );
        assert_eq!(decision.class, ScreeningMailClass::AdverseAction);
        assert_eq!(decision.route, TriageRoute::LawyerReview);
        assert!(decision.deadlines.is_empty());
    }

    #[test]
    fn an_adverse_action_notice_carries_both_open_deadlines() {
        let trigger = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap();
        let decision = triage(
            true,
            "Your rental application",
            "We denied your application based on your consumer report.",
            trigger,
        );
        assert_eq!(decision.class, ScreeningMailClass::AdverseAction);
        assert_eq!(decision.route, TriageRoute::OpenDispute);
        assert_eq!(
            decision.deadlines,
            vec![
                StatutoryDeadline::new(DeadlineKind::FcraReinvestigation, trigger),
                StatutoryDeadline::new(DeadlineKind::AdverseActionFreeReport, trigger),
            ]
        );
    }

    #[test]
    fn a_forwarded_report_carries_the_reinvestigation_deadline_only() {
        let trigger = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap();
        let decision = triage(
            true,
            "My screening report",
            "Attached is the tenant screening report the landlord ran.",
            trigger,
        );
        assert_eq!(decision.class, ScreeningMailClass::ReportForwarded);
        assert_eq!(decision.route, TriageRoute::OpenDispute);
        assert_eq!(
            decision.deadlines,
            vec![StatutoryDeadline::new(
                DeadlineKind::FcraReinvestigation,
                trigger,
            )]
        );
    }

    #[test]
    fn fcra_results_classify_by_agency_response() {
        assert_eq!(
            classify_fcra_result("The disputed item has been deleted from your file."),
            FcraDisputeResult::CorrectedOrDeleted,
        );
        assert_eq!(
            classify_fcra_result("We verified the item as accurate; it remains on your report."),
            FcraDisputeResult::VerifiedUnchanged,
        );
        // Ambiguous → treated as unchanged, never reported as fixed.
        assert_eq!(
            classify_fcra_result("Thank you for your dispute."),
            FcraDisputeResult::VerifiedUnchanged,
        );
    }

    #[test]
    fn a_lawsuit_is_referred_out_not_answered() {
        // A summons routes to litigation, and the referral never answers
        // the correspondence — it hands off to litigation counsel.
        let class = classify("Summons", "You are being sued in civil action.");
        assert_eq!(route(class), TriageRoute::ReferLitigation);
        let referral = litigation_referral("a summons was served");
        assert_eq!(referral.counsel_link, "mailto:contact@neonlaw.com");
        assert!(!referral.answered_as_correspondence);
    }

    #[test]
    fn both_statutory_windows_carry_their_length_and_citation() {
        let trigger = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap();
        assert_eq!(
            deadline_from(DeadlineKind::FcraReinvestigation, trigger),
            NaiveDate::from_ymd_opt(2026, 7, 3).unwrap(),
        );
        assert_eq!(
            deadline_from(DeadlineKind::AdverseActionFreeReport, trigger),
            NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
        );
        assert_eq!(
            DeadlineKind::FcraReinvestigation.statute(),
            "15 U.S.C. § 1681i(a)(1)"
        );
        assert_eq!(
            DeadlineKind::AdverseActionFreeReport.statute(),
            "15 U.S.C. § 1681j(b)"
        );
        assert_eq!(DeadlineKind::FcraReinvestigation.days(), 30);
        assert_eq!(DeadlineKind::AdverseActionFreeReport.days(), 60);
        assert_eq!(
            DeadlineKind::FcraReinvestigation.storage_token(),
            "fcra_reinvestigation"
        );
        assert_eq!(
            DeadlineKind::AdverseActionFreeReport.storage_token(),
            "adverse_action_free_report"
        );
    }
}
