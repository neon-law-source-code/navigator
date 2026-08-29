//! Cross-check: every bundled template's frontmatter spec must
//! match the standalone `workflows/specs/<code>.yaml`.
//!
//! The template markdown still carries an authoritative copy of the
//! `workflow:` and `questionnaire:` blocks (rendered alongside the
//! body, validated by `workflow_integrity`). The standalone YAML is
//! the format `cli scaffold` will generate; it has to stay in lockstep
//! with the markdown until we delete the duplicated frontmatter.
//! Project-scoped operational specs may be bundled without a catalog
//! template, but every other missing template fails fast.

use std::path::{Path, PathBuf};

use workflows::{
    custom_questions_from_template, custom_questions_from_yaml, prompt_overrides_from_template,
    prompt_overrides_from_yaml, questionnaire_spec_from_template, questionnaire_spec_from_yaml,
    workflow_spec_from_template, workflow_spec_from_yaml, BUNDLED_SPEC_YAML,
};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workflows crate lives one level below the workspace root")
        .join("templates")
}

fn read_template_for(code: &str) -> Option<String> {
    let root = templates_root();
    for entry in walk_markdown(&root) {
        let md = std::fs::read_to_string(&entry)
            .unwrap_or_else(|e| panic!("read {}: {e}", entry.display()));
        if !md.starts_with("---\n") {
            continue;
        }
        // Match on the `code:` line in the frontmatter.
        if md.contains(&format!("code: {code}\n")) {
            return Some(md);
        }
    }
    None
}

fn walk_markdown(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_bundled_spec_yaml_matches_its_template_frontmatter() {
    for (code, yaml) in BUNDLED_SPEC_YAML {
        let Some(markdown) = read_template_for(code) else {
            assert_eq!(
                *code, "memo__contract_review",
                "no template found for code `{code}`",
            );
            continue;
        };

        let from_yaml = workflow_spec_from_yaml(yaml)
            .unwrap_or_else(|e| panic!("standalone yaml for `{code}` failed to parse: {e}"));
        let from_template = workflow_spec_from_template(&markdown)
            .unwrap_or_else(|e| panic!("template frontmatter for `{code}` failed to parse: {e}"));
        assert_eq!(
            from_yaml, from_template,
            "workflow spec mismatch between standalone yaml and template frontmatter for `{code}`",
        );

        let q_from_yaml = questionnaire_spec_from_yaml(yaml).unwrap_or_else(|e| {
            panic!("standalone questionnaire yaml for `{code}` failed to parse: {e}")
        });
        let q_from_template = questionnaire_spec_from_template(&markdown).unwrap_or_else(|e| {
            panic!("template questionnaire frontmatter for `{code}` failed to parse: {e}")
        });
        assert_eq!(
            q_from_yaml, q_from_template,
            "questionnaire mismatch between standalone yaml and template frontmatter for `{code}`",
        );

        let prompts_from_yaml = prompt_overrides_from_yaml(yaml).unwrap_or_else(|e| {
            panic!("standalone prompts yaml for `{code}` failed to parse: {e}")
        });
        let prompts_from_template = prompt_overrides_from_template(&markdown).unwrap_or_else(|e| {
            panic!("template prompts frontmatter for `{code}` failed to parse: {e}")
        });
        assert_eq!(
            prompts_from_yaml, prompts_from_template,
            "prompts mismatch between standalone yaml and template frontmatter for `{code}`",
        );

        let cq_from_yaml = custom_questions_from_yaml(yaml).unwrap_or_else(|e| {
            panic!("standalone custom_questions yaml for `{code}` failed to parse: {e}")
        });
        let cq_from_template = custom_questions_from_template(&markdown).unwrap_or_else(|e| {
            panic!("template custom_questions frontmatter for `{code}` failed to parse: {e}")
        });
        assert_eq!(
            cq_from_yaml, cq_from_template,
            "custom_questions mismatch between standalone yaml and template frontmatter for `{code}`",
        );
    }
}
