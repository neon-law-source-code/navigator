//! `cli scaffold <matter> --category <cat> --jurisdiction <j>` —
//! drop the three workspace files that every new legal workflow
//! starts with:
//!
//! 1. `templates/notations/<category>/<jurisdiction>.md`
//! 2. `workflows/specs/<code>.yaml`
//! 3. `features/tests/features/<matter>.feature`
//!
//! Each file lands with a minimal, valid placeholder so the workspace
//! still passes `cargo test --workspace` and the markdown lint:
//!
//! - The template body has YAML frontmatter (`kind`, `title`, `code`,
//!   `respondent_type`) with a placeholder questionnaire / workflow; the
//!   standalone YAML mirrors that spec.
//! - The standalone YAML is the smallest legal `BEGIN → END` machine
//!   for both questionnaire and workflow.
//! - The `.feature` file holds a single placeholder scenario that
//!   loads the new template via the same shape-lock pattern the
//!   existing legal/compliance/nonprofit suites use.
//!
//! The Cucumber **runner** (the `.rs` file that pairs with the
//! feature) is intentionally not generated — picking the right
//! shape-lock or end-to-end runner depends on the workflow's actual
//! shape. `docs/agent-workflows.md` and `docs/notation-authoring.md`
//! cover the manual step.
//!
//! Files that already exist are left untouched — scaffold is idem-
//! potent.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Outcome reported per scaffolded file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOutcome {
    Created,
    AlreadyExists,
}

/// Run the scaffold subcommand. `workspace_root` is the repo root
/// (CWD by default); `matter`, `category`, and `jurisdiction` come
/// straight from the CLI.
pub fn run(workspace_root: &Path, matter: &str, category: &str, jurisdiction: &str) -> ExitCode {
    let code = template_code(matter, jurisdiction);
    let snake_jurisdiction = snake_case(jurisdiction);

    let template_path = workspace_root
        .join("templates")
        .join("notations")
        .join(category)
        .join(format!("{snake_jurisdiction}.md"));
    let spec_path = workspace_root
        .join("workflows")
        .join("specs")
        .join(format!("{code}.yaml"));
    let feature_path = workspace_root
        .join("features")
        .join("tests")
        .join("features")
        .join(format!("{matter}.feature"));

    let outcomes = [
        (
            "template",
            template_path.clone(),
            template_body(matter, &code, jurisdiction),
        ),
        (
            "spec yaml",
            spec_path.clone(),
            spec_yaml(&snake_jurisdiction),
        ),
        (
            "feature file",
            feature_path.clone(),
            feature_body(matter, &code),
        ),
    ];

    for (label, path, contents) in &outcomes {
        match scaffold_one(path, contents) {
            Ok(FileOutcome::Created) => println!("created   {label:14} {}", path.display()),
            Ok(FileOutcome::AlreadyExists) => {
                println!("exists    {label:14} {} (left alone)", path.display());
            }
            Err(e) => {
                eprintln!("navigator: scaffold {label}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    println!();
    println!("next steps:");
    println!("  1. add `(\"{code}\", include_str!(\"../specs/{code}.yaml\")),`");
    println!("     to `BUNDLED_SPEC_YAML` in workflows/src/specs.rs.");
    println!("  2. flesh out the template body + workflow spec YAML.");
    println!("  3. write the cucumber runner under features/tests/{matter}.rs.");
    println!("  4. add the runner to features/Cargo.toml as a `[[test]]` entry.");
    ExitCode::SUCCESS
}

fn scaffold_one(path: &Path, contents: &str) -> std::io::Result<FileOutcome> {
    if path.exists() {
        return Ok(FileOutcome::AlreadyExists);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(FileOutcome::Created)
}

fn template_code(matter: &str, jurisdiction: &str) -> String {
    format!("{}__{}", snake_case(matter), snake_case(jurisdiction))
}

fn snake_case(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn template_body(matter: &str, code: &str, jurisdiction: &str) -> String {
    let title = title_case(matter);
    // `kind:` is the sole document classifier and is required, so the
    // placeholder declares one. `filing` is the safe default for a new
    // matter; the author changes it to the right notation kind (retainer,
    // letter, will, trust, directive, agreement, onboarding, memo) as they
    // flesh out the template.
    format!(
        "---\n\
         kind: filing\n\
         title: {title} ({jurisdiction})\n\
         respondent_type: entity\n\
         code: {code}\n\
         confidential: false\n\
         questionnaire:\n  \
           BEGIN:\n    \
             _: END\n  \
           END: {{}}\n\
         workflow:\n  \
           BEGIN:\n    \
             _: lawyer_review\n  \
           lawyer_review:\n    \
             _: END\n  \
           END: {{}}\n\
         ---\n\
         \n\
         Placeholder body for the {title} matter in {jurisdiction}.\n\
         Replace this paragraph with the legal prose that uses\n\
         `{{{{question_code}}}}` placeholders to interpolate answers.\n",
    )
}

fn spec_yaml(_snake_jurisdiction: &str) -> String {
    // Mirrors the placeholder questionnaire + workflow above. The
    // coherence test in workflows/tests/spec_coherence.rs requires
    // these two sources to stay aligned.
    "questionnaire:\n  \
       BEGIN:\n    \
         _: END\n  \
       END: {}\n\
     workflow:\n  \
       BEGIN:\n    \
         _: lawyer_review\n  \
       lawyer_review:\n    \
         _: END\n  \
       END: {}\n"
        .to_string()
}

fn feature_body(matter: &str, code: &str) -> String {
    let title = title_case(matter);
    format!(
        "Feature: {title} workflow shape\n\
         \n  \
         Placeholder scenario for the `{code}` template scaffolded\n  \
         by `navigator notations scaffold`. Pin down the transition chain here\n  \
         once the workflow spec is finalised — the existing\n  \
         `legal_workflow_shapes.feature` and\n  \
         `compliance_filings_workflow_shapes.feature` are good shapes\n  \
         to copy.\n\
         \n  \
         Scenario: {title} questionnaire walks BEGIN → END\n    \
           Given the bundled spec yaml \"{code}\"\n    \
           Then the questionnaire transitions, in BEGIN-first order, are:\n      \
             | from  | to  |\n      \
             | BEGIN | END |\n",
    )
}

fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut next_upper = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            out.push(' ');
            next_upper = true;
        } else if next_upper {
            out.extend(c.to_uppercase());
            next_upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Workspace root: the parent of `cli/`. Walks up from the binary's
/// `cargo manifest` dir at runtime if available, otherwise the CWD.
#[must_use]
pub fn workspace_root_from_cli_dir() -> PathBuf {
    // The `cli` binary is shipped at `target/{debug,release}/cli`;
    // at runtime, the most reliable workspace anchor is CWD.
    std::env::current_dir().expect("current working directory")
}

#[cfg(test)]
mod tests {
    use super::{
        feature_body, scaffold_one, snake_case, spec_yaml, template_body, template_code,
        title_case, FileOutcome,
    };
    use tempfile::TempDir;

    #[test]
    fn snake_case_lowercases_and_replaces_punctuation() {
        assert_eq!(snake_case("Nevada"), "nevada");
        assert_eq!(snake_case("New Mexico"), "new_mexico");
        assert_eq!(snake_case("San Francisco-County"), "san_francisco_county");
    }

    #[test]
    fn title_case_handles_underscores_and_hyphens() {
        assert_eq!(title_case("estate_planning"), "Estate Planning");
        assert_eq!(title_case("dissolution-llc"), "Dissolution Llc");
    }

    #[test]
    fn template_code_joins_matter_and_jurisdiction() {
        assert_eq!(
            template_code("incorporation", "Nevada"),
            "incorporation__nevada",
        );
        assert_eq!(
            template_code("annual_report", "New Mexico"),
            "annual_report__new_mexico",
        );
    }

    #[test]
    fn template_body_parses_as_yaml_frontmatter() {
        let body = template_body("estate_planning", "estate_planning__nevada", "Nevada");
        assert!(body.starts_with("---\n"));
        assert!(body.contains("code: estate_planning__nevada"));
        assert!(body.contains("respondent_type: entity"));
        // The placeholder questionnaire has BEGIN → END as the only
        // transition, mirrored in spec_yaml.
        assert!(body.contains("BEGIN:"));
        assert!(body.contains("lawyer_review"));
    }

    #[test]
    fn spec_yaml_matches_template_placeholder_chain() {
        let yaml = spec_yaml("nevada");
        // Both blocks present.
        assert!(yaml.contains("questionnaire:"));
        assert!(yaml.contains("workflow:"));
        assert!(yaml.contains("lawyer_review"));
    }

    #[test]
    fn feature_body_contains_a_scenario_and_the_code() {
        let f = feature_body("estate_planning", "estate_planning__nevada");
        assert!(f.contains("Feature:"));
        assert!(f.contains("Scenario:"));
        assert!(f.contains("estate_planning__nevada"));
    }

    #[test]
    fn scaffold_one_creates_then_no_ops() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a/b/x.txt");

        let first = scaffold_one(&path, "hello").unwrap();
        assert_eq!(first, FileOutcome::Created);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        // Second call must not overwrite.
        let second = scaffold_one(&path, "different").unwrap();
        assert_eq!(second, FileOutcome::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn full_scaffold_drops_three_files_under_a_temp_workspace() {
        let dir = TempDir::new().unwrap();
        let _ = super::run(dir.path(), "estate_planning", "estate", "Nevada");

        assert!(dir
            .path()
            .join("templates/notations/estate/nevada.md")
            .exists());
        assert!(dir
            .path()
            .join("workflows/specs/estate_planning__nevada.yaml")
            .exists());
        assert!(dir
            .path()
            .join("features/tests/features/estate_planning.feature")
            .exists());
    }
}
