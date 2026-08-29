//! Cucumber runner for `features/nautilus_workflows.feature`.
//!
//! Pins inbound screening-mail triage and the litigation boundary so a lawsuit
//! is referred out rather than answered as correspondence.

#![allow(clippy::unused_async)]
#![allow(clippy::missing_fields_in_debug)]

use cucumber::{given, then, World};
use workflows::{
    classify, classify_fcra_result, litigation_referral, route, triage, FcraDisputeResult,
    ScreeningMailClass, TriageRoute,
};

#[derive(Default, World)]
#[world(init = Self::default)]
struct NautilusWorld {
    inbound_text: Option<String>,
    has_active_matter: bool,
    reinvestigation_text: Option<String>,
}

impl std::fmt::Debug for NautilusWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NautilusWorld")
            .field("has_inbound_text", &self.inbound_text.is_some())
            .field("has_active_matter", &self.has_active_matter)
            .finish()
    }
}

fn fcra_name(result: FcraDisputeResult) -> &'static str {
    match result {
        FcraDisputeResult::CorrectedOrDeleted => "CorrectedOrDeleted",
        FcraDisputeResult::VerifiedUnchanged => "VerifiedUnchanged",
    }
}

fn class_name(class: ScreeningMailClass) -> &'static str {
    match class {
        ScreeningMailClass::LawsuitOrSummons => "LawsuitOrSummons",
        ScreeningMailClass::ReinvestigationResult => "ReinvestigationResult",
        ScreeningMailClass::AdverseAction => "AdverseAction",
        ScreeningMailClass::ReportForwarded => "ReportForwarded",
        ScreeningMailClass::Other => "Other",
    }
}

fn route_name(route: TriageRoute) -> &'static str {
    match route {
        TriageRoute::ReferLitigation => "ReferLitigation",
        TriageRoute::OpenDispute => "OpenDispute",
        TriageRoute::ReinvestigationReview => "ReinvestigationReview",
        TriageRoute::LawyerReview => "LawyerReview",
    }
}

#[given(regex = r#"^an inbound screening email on an active matter saying "([^"]*)"$"#)]
async fn inbound_on_active_matter(world: &mut NautilusWorld, text: String) {
    world.inbound_text = Some(text);
    world.has_active_matter = true;
}

#[given(regex = r#"^an inbound screening email with no matching matter saying "([^"]*)"$"#)]
async fn inbound_unmatched(world: &mut NautilusWorld, text: String) {
    world.inbound_text = Some(text);
    world.has_active_matter = false;
}

#[then(regex = r#"^it is classified as "([^"]+)" and routed to "([^"]+)"$"#)]
async fn assert_class_and_route(world: &mut NautilusWorld, class: String, route_to: String) {
    let text = world.inbound_text.as_ref().expect("inbound text set");
    let actual_class = classify("", text);
    assert_eq!(class_name(actual_class), class, "classification mismatch");
    assert_eq!(route_name(route(actual_class)), route_to, "route mismatch");
}

#[then(regex = r#"^it is routed to "([^"]+)"$"#)]
async fn assert_route_only(world: &mut NautilusWorld, route_to: String) {
    let text = world.inbound_text.as_ref().expect("inbound text set");
    let decision = triage(
        world.has_active_matter,
        "",
        text,
        chrono::NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
    );
    assert_eq!(route_name(decision.route), route_to, "route mismatch");
}

#[given(regex = r#"^a consumer reporting agency reinvestigation response saying "([^"]*)"$"#)]
async fn fcra_response(world: &mut NautilusWorld, text: String) {
    world.reinvestigation_text = Some(text);
}

#[then(regex = r#"^the FCRA result is "([^"]+)"$"#)]
async fn assert_fcra_result(world: &mut NautilusWorld, result: String) {
    let text = world
        .reinvestigation_text
        .as_ref()
        .expect("reinvestigation text set");
    assert_eq!(
        fcra_name(classify_fcra_result(text)),
        result,
        "FCRA result mismatch"
    );
}

#[then(
    regex = r#"^the litigation referral links to "([^"]+)" and is not answered as correspondence$"#
)]
async fn assert_referral(_world: &mut NautilusWorld, link: String) {
    let referral = litigation_referral("a summons was served");
    assert_eq!(referral.counsel_link, link, "referral link mismatch");
    assert!(
        !referral.answered_as_correspondence,
        "a referred lawsuit must never be answered as correspondence"
    );
}

#[tokio::main]
async fn main() {
    NautilusWorld::cucumber()
        .run_and_exit("tests/features/nautilus_workflows.feature")
        .await;
}
