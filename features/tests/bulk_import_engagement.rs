//! Cucumber runner for `features/bulk_import_engagement.feature`.
//!
//! The bulk-import journey: a lawyer runs a list of organizations
//! and contacts through the one shared `import` engine (the surface the
//! CLI and the AIDA bulk-import tool both call), then opens a matter for
//! one of the imported people via the admin walker. It proves the seam
//! between the import engine and the engagement flow: the imported person
//! becomes the matter's respondent with no re-keying.

// Cucumber's step-attribute macros require `async fn`, so assertion
// steps that don't await anything still have to be declared async.
#![allow(clippy::unused_async)]

use cucumber::{given, then, when, World};
use features::journey::Journey;
use uuid::Uuid;

/// One realistic book of business: two Nevada LLCs and three contacts,
/// each linked to an organization by `organization` key. Zodiac personas,
/// per the firm's fixture convention.
const PAYLOAD: &str = r#"{
  "version": 1,
  "source": "feature-suite",
  "organizations": [
    {"key": "twin-stars", "name": "Twin Stars Co", "entity_type": "Multi Member LLC", "jurisdiction": "NV"},
    {"key": "ram-labs", "name": "Ram Labs", "entity_type": "Single Member LLC", "jurisdiction": "NV"}
  ],
  "people": [
    {"key": "gemini", "name": "Gemini", "email": "gemini@example.com", "title": "Founder", "organization": "twin-stars", "entity_role": "client_contact"},
    {"key": "taurus", "name": "Taurus", "email": "taurus@example.com", "organization": "twin-stars", "entity_role": "client_contact"},
    {"key": "aries", "name": "Aries", "email": "aries@example.com", "organization": "ram-labs", "entity_role": "client_contact"}
  ]
}"#;

#[derive(Default, World)]
#[world(init = Self::default)]
struct BulkWorld {
    journey: Option<Journey>,
    report: Option<import::ImportReport>,
    gemini_id: Option<Uuid>,
    twin_stars_id: Option<Uuid>,
    notation_id: Option<Uuid>,
}

impl std::fmt::Debug for BulkWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkWorld")
            .field("gemini_id", &self.gemini_id)
            .field("notation_id", &self.notation_id)
            .finish_non_exhaustive()
    }
}

impl BulkWorld {
    fn journey(&self) -> &Journey {
        self.journey.as_ref().expect("journey not built")
    }

    fn report(&self) -> &import::ImportReport {
        self.report.as_ref().expect("import not run")
    }
}

#[given("a fresh Neon Law Navigator app with the canonical templates seeded")]
async fn build_app(world: &mut BulkWorld) {
    world.journey = Some(Journey::open("bulk-import").await);
}

#[when("a lawyer bulk-imports two organizations and three contacts")]
async fn bulk_import(world: &mut BulkWorld) {
    let payload = import::parse(PAYLOAD).expect("payload parses");
    // No structural errors before we write anything.
    let diagnostics = import::validate(&payload);
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.severity == import::Severity::Error),
        "import payload has errors: {diagnostics:?}",
    );
    let report = import::apply(&world.journey().surreal, &payload)
        .await
        .expect("apply import");
    // Capture the ids the engine minted for the rows we engage later.
    world.gemini_id = report
        .people
        .iter()
        .find(|r| r.key == "gemini")
        .and_then(|r| r.id);
    world.twin_stars_id = report
        .organizations
        .iter()
        .find(|r| r.key == "twin-stars")
        .and_then(|r| r.id);
    world.report = Some(report);
}

#[then("the import succeeds with no errors")]
async fn assert_no_failures(world: &mut BulkWorld) {
    assert!(
        !world.report().has_errors(),
        "import reported failures: {:?}",
        world.report(),
    );
}

#[then(regex = r"^(\d+) organizations and (\d+) contacts are created$")]
async fn assert_created(world: &mut BulkWorld, orgs: usize, people: usize) {
    let r = world.report();
    assert_eq!(
        r.organizations
            .iter()
            .filter(|o| o.status == import::Outcome::Created)
            .count(),
        orgs,
        "organizations created",
    );
    assert_eq!(
        r.people
            .iter()
            .filter(|p| p.status == import::Outcome::Created)
            .count(),
        people,
        "contacts created",
    );
}

#[then(regex = r#"^the contact "([^"]+)" is linked to their organization$"#)]
async fn assert_link(world: &mut BulkWorld, email: String) {
    // Resolve the contact the way any surface would — by their email.
    let person = store::persons::find_by_email_ci(&world.journey().surreal, &email)
        .await
        .expect("query person")
        .expect("imported contact exists");
    let person_id = person.id;
    let entity_id = world.twin_stars_id.expect("twin-stars imported");
    let link = store::entity_roles::find(
        &world.journey().surreal,
        person_id,
        entity_id,
        "client_contact",
    )
    .await
    .expect("query link");
    let link = link.expect("imported contact is linked to their organization");
    assert_eq!(link.role, "client_contact");
}

#[when(regex = r#"^the firm opens the "([^"]+)" matter for the imported contact "([^"]+)"$"#)]
async fn open_matter(world: &mut BulkWorld, code: String, email: String) {
    let body = format!(
        "client_email={}&retainer_template_code={code}",
        features::form_encode(&email),
    );
    let resp = world
        .journey()
        .lawyer_post("/app/lawyer/retainers/new", body)
        .await;
    let location = resp.location.unwrap_or_else(|| {
        panic!(
            "opening the matter did not redirect (status {})",
            resp.status
        )
    });
    let id = location
        .strip_prefix("/app/lawyer/notations/")
        .and_then(|s| s.strip_suffix("/step"))
        .unwrap_or_else(|| panic!("unexpected redirect target: {location}"));
    world.notation_id = Some(Uuid::parse_str(id).expect("notation id is a UUID"));
}

#[then("the matter is bound to the imported contact")]
async fn assert_bound(world: &mut BulkWorld) {
    let notation = store::notations::find_by_id(
        &world.journey().surreal,
        world.notation_id.expect("notation"),
    )
    .await
    .expect("query notation")
    .expect("notation exists");
    assert_eq!(
        notation.person_id,
        world.gemini_id.expect("gemini imported"),
        "the matter should reuse the imported person, not create a new one",
    );
}

#[tokio::main]
async fn main() {
    BulkWorld::cucumber()
        .run_and_exit("tests/features/bulk_import_engagement.feature")
        .await;
}
