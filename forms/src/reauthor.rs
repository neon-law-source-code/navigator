//! Re-authoring plans and map-less filling (#256 item 1).
//!
//! A re-authored blank's `AcroForm` `/T` names *are* questionnaire state
//! paths, so the `.fields.toml` indirection retires: [`plan`] turns a
//! form's existing map — the recorded human judgment about every hostile
//! `OmniForm` name — into the exact field-layer transformation
//! `pdf::reauthor` applies, and [`resolve_reauthored`] fills straight
//! from the `/T` names afterwards. The repo keeps a diffable `.fields`
//! manifest (one `/T` per line, [`manifest`]) as the offline mirror of
//! the re-authored layer; the `.sha256` pin ties it to the exact bucket
//! bytes.
//!
//! Deliberately-unmapped fields (payment pages, the Registered Agent
//! Acceptance page, the shared-widget quirks) don't disappear — they are
//! renamed into the [`UNMAPPED_PREFIX`] namespace, so "unmapped" is an
//! explicit, guard-checked decision recorded in the bytes themselves.

use std::collections::{BTreeMap, BTreeSet};

use crate::fieldmap::{FieldMap, FieldMapError, FieldRule, PersonRow};

/// The reserved `/T` namespace for fields the fill path never touches.
pub const UNMAPPED_PREFIX: &str = "unmapped__";

/// The bundled `.fields` manifests for re-authored forms (no
/// `.fields.toml` anymore), keyed by `form_code`. One canonical `/T`
/// name per line, sorted — the diffable mirror of the blank's field
/// layer, tied to the exact bytes by the sibling `.sha256` pin.
const BUNDLED_MANIFESTS: &[(&str, &str)] = &[
    (
        "nv__llc_formation",
        include_str!("../../templates/notations/forms/united_states/nevada/state/nv__llc_formation.fields"),
    ),
    (
        "nv__profit_corp_formation",
        include_str!(
            "../../templates/notations/forms/united_states/nevada/state/nv__profit_corp_formation.fields"
        ),
    ),
    (
        "nv__business_trust_formation",
        include_str!(
            "../../templates/notations/forms/united_states/nevada/state/nv__business_trust_formation.fields"
        ),
    ),
    (
        "us__naturalization",
        include_str!("../../templates/notations/forms/united_states/federal/uscis/us__naturalization.fields"),
    ),
];

/// The re-authored field manifest for one form, or `None` when the form
/// still fills through a `.fields.toml`.
#[must_use]
pub fn manifest(form_code: &str) -> Option<Vec<&'static str>> {
    BUNDLED_MANIFESTS
        .iter()
        .find(|(code, _)| *code == form_code)
        .map(|(_, raw)| raw.lines().filter(|l| !l.is_empty()).collect())
}

/// The field-layer transformation [`plan`] computes — the forms-side
/// mirror of `pdf::ReauthorSpec`, kept here so this crate stays free of
/// the `pdf` dependency (the CLI converts).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Old `/T` → new `/T`. Several olds may share one new name (the
    /// packets restate the same person on several pages); the transform
    /// merges those into one multi-widget field.
    pub renames: BTreeMap<String, String>,
    /// Radio merges: canonical group state → the member checkboxes'
    /// old `/T` names and final export states.
    pub radios: BTreeMap<String, Vec<RadioMember>>,
    /// Old `/T` → fixed value, pre-printed as static content.
    pub literals: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioMember {
    pub field: String,
    pub source_export: String,
    pub final_export: String,
}

/// Errors turning a `.fields.toml` into a re-author [`Plan`].
#[derive(Debug, thiserror::Error)]
pub enum ReauthorPlanError {
    /// A rule references a question no questionnaire state declares —
    /// caught here, before any bytes change, so the re-authored `/T`
    /// name can never point at a path the questionnaire won't produce.
    #[error("field `{field}`: question `{question}` resolves to no declared questionnaire state")]
    UnresolvedQuestion { field: String, question: String },
    /// A rule shape that cannot become a bare `/T` name (`value_map`,
    /// `present_in`). The judgment it encodes must be re-expressed in
    /// the map first — e.g. a derived slot label becomes the person
    /// row's own `title` part — so the adjudication stays reviewable.
    #[error("field `{field}`: {reason}")]
    UnsupportedRule { field: String, reason: String },
    /// Two plan entries would mint the same final `/T` name — a rename
    /// target colliding with a radio group. Two fields sharing a name
    /// fill as one: the other is a silent blank box on a filed form.
    #[error(
        "final `/T` `{name}` is minted by both a rename target and a radio group; \
         two fields sharing a name fill as one and leave the other a silent blank box"
    )]
    DuplicateFinalName { name: String },
    /// A checkbox condition the transform cannot keep truthful. The
    /// merged radio group's final export values are the map's
    /// `checked_when` answers, while each member still records the
    /// source PDF's original `on_state` so its appearance stream can be
    /// preserved. Each answer must be unique within its group, and
    /// neither half of the pair may ride alone.
    #[error("field `{field}`: {reason}")]
    InvalidCheckbox { field: String, reason: String },
    /// A rename target carries an all-digit dotted segment that no
    /// `row`/`part` list rule minted. `resolve_reauthored` claims every
    /// `<state>.<n>.<part>` shape as a people-row slot, so such a field
    /// would silently never fill.
    #[error(
        "field `{field}`: target `{target}` carries numeric segment `{segment}` outside a \
         `row`/`part` list rule — the fill path would claim it as a people-row slot and \
         silently leave it blank"
    )]
    NumericSegment {
        field: String,
        target: String,
        segment: String,
    },
}

/// Turn a form's `.fields.toml` (the recorded mapping judgment) plus the
/// blank's actual field names into the transformation that makes the
/// `/T` names *be* questionnaire state paths:
///
/// - `question` rules rename to the canonical state (plus `.row.part`
///   for people-list slots) — several olds may share one target;
/// - `checked_when` pairs group into one radio per canonical state;
/// - `literal` rules pre-print;
/// - every blank field the map does not cover renames into the
///   [`UNMAPPED_PREFIX`] namespace, so leaving it unfilled is an
///   explicit decision the guard can check.
///
/// # Errors
///
/// [`ReauthorPlanError`] — an unresolvable question reference or a rule
/// shape (`value_map` / `present_in`) that must be re-expressed first.
pub fn plan(
    map: &FieldMap,
    blank_field_names: &[String],
    states: &[String],
) -> Result<Plan, ReauthorPlanError> {
    let states: BTreeSet<&str> = states.iter().map(String::as_str).collect();
    let mut out = Plan::default();
    // Per radio group: answer key → the member that claimed it, so a
    // second claim can be refused by name.
    let mut radio_states: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for rule in &map.field {
        if let Some(literal) = &rule.literal {
            out.literals.insert(rule.name.clone(), literal.clone());
            continue;
        }
        if rule.value_map.is_some() || rule.present_in.is_some() {
            return Err(ReauthorPlanError::UnsupportedRule {
                field: rule.name.clone(),
                reason: "`value_map` / `present_in` rules cannot become a `/T` name; \
                         re-express the judgment in the map first (a derived slot label \
                         becomes the person row's own `title` part)"
                    .into(),
            });
        }
        let question = rule.question.as_deref().unwrap_or_default();
        let Some(target) = canonical_target(question, &states) else {
            return Err(ReauthorPlanError::UnresolvedQuestion {
                field: rule.name.clone(),
                question: question.to_string(),
            });
        };
        if let Some(segment) = numeric_segment(&target) {
            return Err(ReauthorPlanError::NumericSegment {
                field: rule.name.clone(),
                segment: segment.to_string(),
                target,
            });
        }
        if let Some((answer, on_state)) = checkbox_state(rule)? {
            if !radio_states
                .entry(target.clone())
                .or_default()
                .insert(answer.clone())
            {
                return Err(ReauthorPlanError::InvalidCheckbox {
                    field: rule.name.clone(),
                    reason: format!(
                        "answer `{answer}` is already claimed by another member of \
                         radio `{target}`; an ambiguous state would check the wrong box"
                    ),
                });
            }
            out.radios.entry(target).or_default().push(RadioMember {
                field: rule.name.clone(),
                source_export: on_state,
                final_export: answer,
            });
            continue;
        }
        let target = match (rule.row, rule.part.as_deref()) {
            (Some(row), Some(part)) => format!("{target}.{row}.{part}"),
            _ => target,
        };
        out.renames.insert(rule.name.clone(), target);
    }

    let covered: BTreeSet<String> = out
        .renames
        .keys()
        .chain(out.literals.keys())
        .chain(out.radios.values().flatten().map(|member| &member.field))
        .cloned()
        .collect();
    for name in blank_field_names {
        if !covered.contains(name) {
            out.renames
                .insert(name.clone(), format!("{UNMAPPED_PREFIX}{name}"));
        }
    }

    let rename_targets: BTreeSet<&String> = out.renames.values().collect();
    if let Some(name) = out.radios.keys().find(|n| rename_targets.contains(n)) {
        return Err(ReauthorPlanError::DuplicateFinalName { name: name.clone() });
    }
    Ok(out)
}

/// The first all-digit dotted segment of `target`, if any — the shape
/// only a `row`/`part` list rule may mint (it appends `.row.part`
/// *after* this check runs on the question-derived target).
fn numeric_segment(target: &str) -> Option<&str> {
    target
        .split('.')
        .find(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

/// Validate a rule's checkbox condition: `Some((answer, on_state))` for a
/// well-formed `checked_when`/`on_state` pair, `None` for a plain rule.
/// The merged radio group's final export values are the answers, while
/// the source on-states are retained long enough for `pdf::reauthor` to
/// rename each widget appearance stream. Neither half may ride alone
/// because it would silently become a plain text rename.
fn checkbox_state(rule: &FieldRule) -> Result<Option<(String, String)>, ReauthorPlanError> {
    match (&rule.checked_when, &rule.on_state) {
        (None, None) => Ok(None),
        (Some(when), Some(state)) => Ok(Some((when.clone(), state.clone()))),
        (Some(_), None) | (None, Some(_)) => {
            let (present, absent) = if rule.checked_when.is_some() {
                ("checked_when", "on_state")
            } else {
                ("on_state", "checked_when")
            };
            Err(ReauthorPlanError::InvalidCheckbox {
                field: rule.name.clone(),
                reason: format!(
                    "`{present}` without `{absent}` is half a checkbox condition — it \
                     would silently become a plain text rename"
                ),
            })
        }
    }
}

/// Canonicalize a map question reference against the declared
/// questionnaire states: the head (before any dotted tail) either *is* a
/// state or is the `__role` suffix of exactly one — the same resolution
/// the #255 guard uses. The dotted tail rides along (`entity__company.name`
/// stays itself).
fn canonical_target(question: &str, states: &BTreeSet<&str>) -> Option<String> {
    let (head, tail) = question
        .split_once('.')
        .map_or((question, None), |(h, t)| (h, Some(t)));
    let canonical_head = if states.contains(head) {
        head.to_string()
    } else {
        let mut suffixed = states
            .iter()
            .filter(|s| s.strip_suffix(head).is_some_and(|p| p.ends_with("__")));
        let first = (*suffixed.next()?).to_string();
        if suffixed.next().is_some() {
            return None;
        }
        first
    };
    Some(match tail {
        Some(tail) => format!("{canonical_head}.{tail}"),
        None => canonical_head,
    })
}

/// Fill values for a re-authored blank: every `/T` name *is* its data
/// path, so resolution needs no map — `<state>.<row>.<part>` indexes a
/// people-list answer, any other dotted or bare name is an exact answer
/// key, and [`UNMAPPED_PREFIX`] names are skipped (payment pages and
/// lawyer-side artifacts never fill). Missing or empty answers skip their
/// fields, mirroring the mapped `resolve`.
///
/// # Errors
///
/// [`FieldMapError::MalformedPeopleList`] when a row-indexed name's
/// answer is not a JSON array of person rows.
pub fn resolve_reauthored(
    field_names: &[String],
    answers: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, FieldMapError> {
    resolve_reauthored_with_title_defaults(field_names, answers, &[])
}

pub(crate) fn resolve_reauthored_with_title_defaults(
    field_names: &[String],
    answers: &BTreeMap<String, String>,
    title_defaults: &[(&str, &str)],
) -> Result<BTreeMap<String, String>, FieldMapError> {
    let mut out = BTreeMap::new();
    for name in field_names {
        if name.starts_with(UNMAPPED_PREFIX) {
            continue;
        }
        let segments: Vec<&str> = name.split('.').collect();
        if let [state, row, part] = segments.as_slice() {
            if let Ok(row) = row.parse::<usize>() {
                let Some(answer) = answers.get(*state).filter(|a| !a.is_empty()) else {
                    continue;
                };
                let rows: Vec<PersonRow> = serde_json::from_str(answer).map_err(|source| {
                    FieldMapError::MalformedPeopleList {
                        field: name.clone(),
                        question: (*state).to_string(),
                        source,
                    }
                })?;
                if let Some(person) = rows.get(row) {
                    if let Some(value) = person.part(part).filter(|value| !value.is_empty()) {
                        out.insert(name.clone(), value.to_string());
                    } else if *part == "title" {
                        if let Some((_, default)) =
                            title_defaults.iter().find(|(list, _)| list == state)
                        {
                            out.insert(name.clone(), (*default).to_string());
                        }
                    }
                }
                continue;
            }
        }
        if let Some(value) = answers.get(name).filter(|a| !a.is_empty()) {
            out.insert(name.clone(), value.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{canonical_target, plan, resolve_reauthored, ReauthorPlanError, UNMAPPED_PREFIX};
    use std::collections::{BTreeMap, BTreeSet};

    /// A representative slice of the retired LLC `.fields.toml` — the
    /// rule shapes the planner must keep speaking (literal, identity
    /// rename, checkbox pair, people rows, and the organizer block that
    /// restates slot 0 under different `/T` names).
    fn fixture_map() -> crate::FieldMap {
        toml::from_str(
            r#"
            form_code = "nv__llc_formation"
            [[field]]
            name = "formation_1"
            literal = "NRS 86"
            [[field]]
            name = "1 Name of Entity If foreign name in home jurisdiction"
            question = "entity__company.name"
            [[field]]
            name = "managers_a"
            question = "management_structure"
            checked_when = "managers"
            on_state = "managers"
            [[field]]
            name = "managers_b"
            question = "management_structure"
            checked_when = "members"
            on_state = "members"
            [[field]]
            name = "Name3"
            question = "managing_members"
            row = 0
            part = "name"
            [[field]]
            name = "undefined"
            question = "managing_members"
            row = 0
            part = "street"
            [[field]]
            name = "Address6"
            question = "managing_members"
            row = 0
            part = "street"
            "#,
        )
        .expect("fixture parses")
    }

    fn llc_states() -> Vec<String> {
        [
            "entity__company",
            "person__registered_agent",
            "custom_single_choice__management_structure",
            "people__managing_members",
        ]
        .iter()
        .map(ToString::to_string)
        .collect()
    }

    #[test]
    fn plans_the_llc_map_into_canonical_targets() {
        let map = fixture_map();
        let blank_names: Vec<String> = map.field.iter().map(|r| r.name.clone()).collect();
        let plan = plan(&map, &blank_names, &llc_states()).expect("plans");

        assert_eq!(
            plan.renames
                .get("1 Name of Entity If foreign name in home jurisdiction"),
            Some(&"entity__company.name".to_string())
        );
        assert_eq!(
            plan.renames.get("Name3"),
            Some(&"people__managing_members.0.name".to_string())
        );
        // The organizer block restates slot-0 — same target, merged.
        assert_eq!(
            plan.renames.get("undefined"),
            Some(&"people__managing_members.0.street".to_string())
        );
        assert_eq!(
            plan.renames.get("Address6"),
            Some(&"people__managing_members.0.street".to_string())
        );
        assert_eq!(
            plan.radios
                .get("custom_single_choice__management_structure")
                .map(|members| members
                    .iter()
                    .map(|member| (
                        member.field.as_str(),
                        member.source_export.as_str(),
                        member.final_export.as_str()
                    ))
                    .collect::<Vec<_>>()),
            Some(vec![
                ("managers_a", "managers", "managers"),
                ("managers_b", "members", "members")
            ])
        );
        assert_eq!(
            plan.literals.get("formation_1"),
            Some(&"NRS 86".to_string())
        );
    }

    #[test]
    fn uncovered_blank_fields_land_in_the_unmapped_namespace() {
        let map = fixture_map();
        let mut blank_names: Vec<String> = map.field.iter().map(|r| r.name.clone()).collect();
        blank_names.push("City".into()); // the shared-widget quirk, deliberately unmapped
        let plan = plan(&map, &blank_names, &llc_states()).expect("plans");
        assert_eq!(
            plan.renames.get("City"),
            Some(&format!("{UNMAPPED_PREFIX}City"))
        );
    }

    #[test]
    fn resolves_reauthored_names_from_answers() {
        let names: Vec<String> = [
            "entity__company.name",
            "custom_single_choice__management_structure",
            "people__managing_members.0.name",
            "people__managing_members.0.title",
            "people__managing_members.2.name", // row the client never gave
            "unmapped__City",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let mut answers = BTreeMap::new();
        answers.insert("entity__company.name".to_string(), "Neon LLC".to_string());
        answers.insert(
            "custom_single_choice__management_structure".to_string(),
            "managers".to_string(),
        );
        answers.insert(
            "people__managing_members".to_string(),
            r#"[{"name":"Ada Member","title":"Manager"}]"#.to_string(),
        );
        answers.insert("unmapped__City".to_string(), "must never fill".to_string());

        let out = resolve_reauthored(&names, &answers).expect("resolves");
        assert_eq!(
            out.get("entity__company.name").map(String::as_str),
            Some("Neon LLC")
        );
        assert_eq!(
            out.get("custom_single_choice__management_structure")
                .map(String::as_str),
            Some("managers")
        );
        assert_eq!(
            out.get("people__managing_members.0.name")
                .map(String::as_str),
            Some("Ada Member")
        );
        assert_eq!(
            out.get("people__managing_members.0.title")
                .map(String::as_str),
            Some("Manager")
        );
        assert!(!out.contains_key("people__managing_members.2.name"));
        assert!(!out.contains_key("unmapped__City"));
    }

    /// Build a one-rule fixture map around the given `[[field]]` TOML
    /// body, sharing the LLC states.
    fn one_rule_map(rule_toml: &str) -> crate::FieldMap {
        toml::from_str(&format!("form_code = \"t\"\n[[field]]\n{rule_toml}"))
            .expect("fixture parses")
    }

    #[test]
    fn plan_refuses_a_radio_name_colliding_with_a_rename_target() {
        // A map that both checks a box for a choice and prints the same
        // choice as text would mint two final fields with one name — the
        // silent-blank-box collision the pdf side also refuses.
        let map: crate::FieldMap = toml::from_str(
            r#"
            form_code = "t"
            [[field]]
            name = "choice as text"
            question = "management_structure"
            [[field]]
            name = "managers_a"
            question = "management_structure"
            checked_when = "managers"
            on_state = "managers"
            [[field]]
            name = "managers_b"
            question = "management_structure"
            checked_when = "members"
            on_state = "members"
            "#,
        )
        .expect("fixture parses");
        let blank_names: Vec<String> = map.field.iter().map(|r| r.name.clone()).collect();
        let err = plan(&map, &blank_names, &llc_states()).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ReauthorPlanError::DuplicateFinalName { ref name }
                    if name == "custom_single_choice__management_structure"
            ),
            "{err}"
        );
    }

    #[test]
    fn plan_records_an_on_state_differing_from_checked_when() {
        let map = one_rule_map(
            r#"
            name = "managers_a"
            question = "management_structure"
            checked_when = "managers"
            on_state = "1"
            "#,
        );
        let names = vec!["managers_a".to_string()];
        let plan = plan(&map, &names, &llc_states()).expect("plans");
        let members = plan
            .radios
            .get("custom_single_choice__management_structure")
            .expect("radio members");
        assert_eq!(members[0].field, "managers_a");
        assert_eq!(members[0].source_export, "1");
        assert_eq!(members[0].final_export, "managers");
    }

    #[test]
    fn plan_refuses_an_on_state_without_checked_when() {
        let map = one_rule_map(
            r#"
            name = "managers_a"
            question = "management_structure"
            on_state = "managers"
            "#,
        );
        let names = vec!["managers_a".to_string()];
        let err = plan(&map, &names, &llc_states()).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ReauthorPlanError::InvalidCheckbox { ref field, .. } if field == "managers_a"
            ),
            "{err}"
        );
    }

    #[test]
    fn plan_refuses_duplicate_member_on_states() {
        // Two members of one group answering to the same state would
        // check both boxes on a fill — a silent wrong-box error.
        let map: crate::FieldMap = toml::from_str(
            r#"
            form_code = "t"
            [[field]]
            name = "managers_a"
            question = "management_structure"
            checked_when = "managers"
            on_state = "managers"
            [[field]]
            name = "managers_b"
            question = "management_structure"
            checked_when = "managers"
            on_state = "managers"
            "#,
        )
        .expect("fixture parses");
        let blank_names: Vec<String> = map.field.iter().map(|r| r.name.clone()).collect();
        let err = plan(&map, &blank_names, &llc_states()).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ReauthorPlanError::InvalidCheckbox { ref field, .. } if field == "managers_b"
            ),
            "{err}"
        );
    }

    #[test]
    fn plan_refuses_numeric_segments_not_minted_by_a_row_rule() {
        // `resolve_reauthored` claims any `<state>.<n>.<part>` shape as
        // a people-row slot, so a numeric segment riding in on a
        // question's dotted tail would silently never fill.
        let map = one_rule_map(
            r#"
            name = "sneaky"
            question = "entity__company.2.name"
            "#,
        );
        let names = vec!["sneaky".to_string()];
        let err = plan(&map, &names, &llc_states()).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ReauthorPlanError::NumericSegment { ref field, ref segment, .. }
                    if field == "sneaky" && segment == "2"
            ),
            "{err}"
        );
    }

    #[test]
    fn canonical_target_resolves_suffixes_and_keeps_dotted_tails() {
        let states = llc_states();
        let states: BTreeSet<&str> = states.iter().map(String::as_str).collect();
        assert_eq!(
            canonical_target("managing_members", &states).as_deref(),
            Some("people__managing_members")
        );
        assert_eq!(
            canonical_target("entity__company.name", &states).as_deref(),
            Some("entity__company.name")
        );
        assert_eq!(canonical_target("nonexistent", &states), None);
    }
}
