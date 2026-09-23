//! End-to-end tests for `navigator glossary ...`.

use std::process::Command;

use assert_cmd::cargo::cargo_bin;

#[test]
fn the_glossary_names_the_physical_schema() {
    let glossary = glossary_text();
    let schema = include_str!("../../store/src/schema/navigator.surql");

    for table in ["person", "person_project_role", "project"] {
        assert!(
            schema.contains(&format!("DEFINE TABLE IF NOT EXISTS {table} ")),
            "schema must define `{table}`"
        );
    }

    for field in [
        "`person.role`",
        "`person_project_role.participation`",
        "`project.code`",
    ] {
        assert!(glossary.contains(field), "glossary must use `{field}`");
    }

    for retired_name in [
        "`persons.role`",
        "`persons` row",
        "`person_project_roles`",
        "`person_project_roles.participation`",
        "`projects.code`",
    ] {
        assert!(
            !glossary.contains(retired_name),
            "glossary must not name the non-schema `{retired_name}`"
        );
    }
}

/// Every term's title and body, one after another — what a reader of the
/// whole glossary sees, for assertions about its prose.
fn glossary_text() -> String {
    use std::fmt::Write as _;
    store::glossary::terms()
        .iter()
        .fold(String::new(), |mut text, term| {
            let _ = write!(text, "## {}\n\n{}\n\n", term.title, term.body);
            text
        })
}

fn navigator(args: &[&str]) -> std::process::Output {
    Command::new(cargo_bin("navigator"))
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run navigator {args:?}: {error}"))
}

#[test]
fn glossary_requires_a_subcommand() {
    let out = navigator(&["glossary"]);
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Usage: navigator glossary <COMMAND>"),
        "expected glossary subcommand usage, got: {stderr}",
    );
}

#[test]
fn glossary_list_prints_every_term_as_slug_and_title() {
    let out = navigator(&["glossary", "list"]);
    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let terms = store::glossary::terms();
    assert_eq!(lines.len(), terms.len(), "one line per term: {stdout}");
    for (line, term) in lines.iter().zip(terms) {
        assert_eq!(*line, format!("{}\t{}", term.slug, term.title));
    }
    assert!(lines.contains(&"lawyer-review\tLawyer Review"));
    assert!(lines.contains(&"workflow-runtime\tWorkflow Runtime"));
}

#[test]
fn the_retired_docs_command_is_gone() {
    let out = navigator(&["dev", "docs", "list"]);
    assert!(
        !out.status.success(),
        "`dev docs` is replaced by `glossary`"
    );
}

#[test]
fn glossary_show_with_known_term_prints_just_that_term() {
    let out = navigator(&["glossary", "show", "Lawyer Review"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Lawyer Review"));
    assert!(stdout.contains("`lawyer_review`"));
    assert!(stdout.contains("notation-authoring.md"));
    assert!(!stdout.contains("## Workflow Runtime"));
}

/// #539 defines **Deadline** as a first-class term before the schema that
/// carries it. The entry is load-bearing beyond naming: it is the only place
/// the two decisions #688 is gated on are written down — that a due date is
/// stored rather than recomputed at read time, and that internal and client
/// lead times are separate, with the firm warned no later than the client.
/// Assert the vocabulary and both decisions, so a later edit cannot quietly
/// drop them back into an issue comment.
#[test]
fn glossary_deadline_carries_its_authority_vocabulary_and_both_decisions() {
    let out = navigator(&["glossary", "show", "Deadline"]);
    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Deadline"));
    assert!(
        stdout.contains("`project_deadlines`"),
        "the schema-side name is the ruling this entry records, not `matter_deadlines`"
    );
    for authority_kind in [
        "`statute`",
        "`court_rule`",
        "`court_order`",
        "`contract`",
        "`internal`",
    ] {
        assert!(
            stdout.contains(authority_kind),
            "the closed authority_kind vocabulary must list {authority_kind}"
        );
    }
    assert!(
        stdout.contains("Stored, never derived"),
        "the stored-versus-derived decision gates #688 and lives only here"
    );
    assert!(
        stdout.contains("internal lead is never shorter than the client lead"),
        "separate internal and client lead times, firm warned first (#896's survey)"
    );
    assert!(
        stdout.contains("`replay_key`"),
        "the nullable-unique replay key is the upsert-data-loss fix"
    );
    assert!(!stdout.contains("## Deployment Environment"));
}

#[test]
fn glossary_show_is_case_insensitive() {
    let out = navigator(&["glossary", "show", "lawyer review"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Lawyer Review"));
    // One term only — no other heading bleeds in.
    assert!(!stdout.contains("## Workflow Runtime"));
}

#[test]
fn glossary_show_accepts_a_slug() {
    let out = navigator(&["glossary", "show", "lawyer-review"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Lawyer Review"));
}

const PROJECT_LIST_RS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../webapp/src/project_list.rs"
));
const LAWYER_DASHBOARD_RS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../webapp/src/lawyer_dashboard.rs"
));
const ACCESS_MODEL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/access-model.md"
));
const AUTHORIZATION_SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../.agents/skills/authorization-model/SKILL.md"
));

#[test]
fn access_teaching_surfaces_keep_route_and_matter_scope_distinct() {
    for stale_phrase in ["admin sees all", "admin sees everything"] {
        assert!(
            !PROJECT_LIST_RS.contains(stale_phrase),
            "project list must not teach the old scope rule: {stale_phrase}"
        );
        assert!(
            !LAWYER_DASHBOARD_RS.contains(stale_phrase),
            "lawyer dashboard must not teach the old scope rule: {stale_phrase}"
        );
    }

    assert!(
        PROJECT_LIST_RS.contains("Every firm tier")
            && PROJECT_LIST_RS.contains("firm-side")
            && PROJECT_LIST_RS.contains("participation row")
            && PROJECT_LIST_RS.contains("matter surface"),
        "project list must distinguish administrative listings from the scoped matter surface"
    );
    assert!(
        LAWYER_DASHBOARD_RS.contains("every firm tier")
            && LAWYER_DASHBOARD_RS.contains("firm-side")
            && LAWYER_DASHBOARD_RS.contains("participation row")
            && LAWYER_DASHBOARD_RS.contains("route")
            && LAWYER_DASHBOARD_RS.contains("admission"),
        "lawyer dashboard must teach participation scope and route admission separately"
    );

    let clerk_section = ACCESS_MODEL_MD
        .split_once("### `clerk`")
        .and_then(|(_, rest)| rest.split_once("### `lawyer`"))
        .map(|(section, _)| section)
        .expect("access model must have a Clerk section between Clerk and Lawyer headings");
    assert!(
        clerk_section.contains("`/app/projects`"),
        "the Clerk lens must be documented on the shared matter route"
    );
    assert!(
        !clerk_section.contains("dedicated `/clerk`")
            && !clerk_section.contains("`/clerk` coordination surface"),
        "the retired dedicated Clerk route must not be documented as live"
    );
    let glossary = glossary_text();
    assert!(
        !glossary.contains("dedicated `/clerk` surface")
            && glossary.contains("Clerk's read-only lens under `/app/projects`"),
        "the glossary must describe the shared Clerk lens"
    );

    let authorization_scope = AUTHORIZATION_SKILL_MD
        .split_once("- **Owner and Admin")
        .and_then(|(_, rest)| rest.split_once("- **`participation`"))
        .map(|(section, _)| section)
        .expect("authorization skill must document the Owner/Admin rule");
    assert!(
        authorization_scope.contains("route admission")
            && authorization_scope.contains("matter surface")
            && authorization_scope.contains("participation"),
        "authorization skill must qualify the Owner/Admin bypass"
    );
}

#[test]
fn glossary_show_unknown_term_exits_non_zero_with_helpful_stderr() {
    let out = navigator(&["glossary", "show", "not-a-real-term"]);
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown term"),
        "expected `unknown term` in stderr, got: {stderr}",
    );
    assert!(
        stderr.contains("Run `navigator glossary list`"),
        "expected hint in stderr, got: {stderr}",
    );
}

/// `docs/notation.md` is a published page, so its Template section is a
/// contract with template authors, not internal prose.
const NOTATION_MD: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/notation.md"));

/// #850 formalizes a Template's four-part model. This guards it against a
/// terseness pass (#859 is running one over `/docs`): the caveat below is
/// the part most likely to read as trimmable filler and is precisely the
/// part that must survive — an author who goes looking for a literal
/// `metadata:` key will not find one, and nothing else in the tree says so.
#[test]
fn notation_docs_state_the_four_part_template_model_and_the_metadata_caveat() {
    let (_, template_section) = NOTATION_MD
        .split_once("## Template")
        .expect("notation.md must document Template");
    let template_section = template_section
        .split_once("\n## ")
        .map_or(template_section, |(section, _)| section);

    assert!(
        template_section.contains("### The four parts"),
        "the four-part model must be an explicit heading, not implied by prose"
    );
    for part in [
        "**Metadata**",
        "**Questionnaire**",
        "**Workflow**",
        "**Body**",
    ] {
        assert!(
            template_section.contains(part),
            "the four-part model must name {part}"
        );
    }
    assert!(
        template_section.contains("Metadata is a conceptual grouping, not a literal YAML key"),
        "the caveat that there is no literal `metadata:` key must survive any edit"
    );
    assert!(
        template_section.contains("rules::frontmatter::field"),
        "each part points at the code that parses it"
    );
}
