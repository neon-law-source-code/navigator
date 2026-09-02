//! The closed question-type registry — the single source of truth for the
//! `<type>` half of a questionnaire state name (`<type>__<role>`).
//!
//! Every type is either **glossary-grounded** (it names the table its
//! answers ground to — a `record` that creates/links a row, or a
//! `reference` that selects a seeded one) or a **custom primitive** (the
//! value lives in the answer JSON, no SQL grounding). Each glossary-grounded type has a
//! **singular** form (one row) and, where a matter collects several, an
//! explicit **plural/aggregate** form (an array of the singular's shape) —
//! `person`→`people`, `entity`→`entities`, and so on. The pairing is
//! explicit because there is no pluralization helper and `person`→`people`
//! is irregular.
//!
//! [`QuestionType`] is that closed set. The guards (`N113`–`N115`), the
//! render/form-fill resolver, and the walkers all read cardinality, shape,
//! and grounding from here rather than re-deriving them — see issue #235.
//! A grounding test pins every record/reference variant to its declared
//! table and bars every deny-listed table.

use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoEnumIterator};

/// Whether a type creates/links a SQL row (`Record`), selects a seeded one
/// (`Reference`), or carries a primitive value with no SQL grounding
/// (`Custom`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// The answer creates or links a `store::entity` row.
    Record,
    /// The answer selects an existing seeded `store::entity` row.
    Reference,
    /// A primitive value living in the answer JSON; no SQL grounding.
    Custom,
}

/// One row (`Singular`) versus many collected under one question
/// (`Aggregate` — the answer JSON is an array of the singular's shape).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cardinality {
    /// A single record/reference/primitive.
    Singular,
    /// A plural collection — an array of the singular's shape. Barred from
    /// `__for_` (its children are inline).
    Aggregate,
}

/// The parts a `people`/`person` aggregate row collects, in canonical
/// order. The single source the widget assembler and the render/fill
/// resolver key on — the aggregate shape is an array of these fields.
pub const PERSON_ROW_PARTS: [&str; 9] = [
    "name", "email", "title", "phone", "street", "city", "state", "zip", "country",
];

/// The closed set of question types — the `<type>` half of a
/// `<type>__<role>` state name. Stored as a string; modelled like
/// [`crate::persons::Role`] so the string form round-trips.
///
/// [`as_str`](Self::as_str) is the single mapping from variant to token,
/// and `EnumIter` is compiler-maintained — so a new variant is a build
/// failure in `as_str` rather than a silently narrowed vocabulary, and
/// [`all_tokens`](Self::all_tokens) picks it up without a second list to
/// keep in sync.
#[derive(Clone, Copy, Debug, Eq, PartialEq, EnumIter, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    // --- Record types (create/link a SQL row) — singular ---
    Person,
    Entity,
    Address,
    Role,
    Filing,
    Credential,
    Disclosure,
    Issuance,
    Signature,
    Notarization,
    // --- Record types — aggregate (array of the singular's shape) ---
    People,
    Entities,
    Addresses,
    Roles,
    Filings,
    Credentials,
    Disclosures,
    Issuances,
    // --- Reference types (select a seeded row) — singular ---
    Jurisdiction,
    /// The `jurisdiction_type = 'country'` subset of the seeded
    /// jurisdictions — a distinct type so a country picker (e.g. an
    /// N-400's country of birth) never offers a U.S. state.
    Country,
    EntityType,
    Project,
    // --- Reference types — aggregate ---
    Jurisdictions,
    EntityTypes,
    // --- Custom primitives (value in the answer JSON, no SQL grounding) ---
    CustomText,
    /// A phone number — the contact primitive (`<input type="tel">`).
    /// The value stays in the answer JSON with the matter; intake
    /// contact facts are matter-scoped, not person-row fields.
    CustomPhone,
    CustomYesNo,
    CustomSingleChoice,
    CustomMultipleChoice,
    CustomUsd,
    CustomDatetime,
}

impl QuestionType {
    /// The `<type>` token as it appears in a state name.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        use QuestionType::{
            Address, Addresses, Country, Credential, Credentials, CustomDatetime,
            CustomMultipleChoice, CustomPhone, CustomSingleChoice, CustomText, CustomUsd,
            CustomYesNo, Disclosure, Disclosures, Entities, Entity, EntityType, EntityTypes,
            Filing, Filings, Issuance, Issuances, Jurisdiction, Jurisdictions, Notarization,
            People, Person, Project, Role, Roles, Signature,
        };
        match self {
            Person => "person",
            Entity => "entity",
            Address => "address",
            Role => "role",
            Filing => "filing",
            Credential => "credential",
            Disclosure => "disclosure",
            Issuance => "issuance",
            Signature => "signature",
            Notarization => "notarization",
            People => "people",
            Entities => "entities",
            Addresses => "addresses",
            Roles => "roles",
            Filings => "filings",
            Credentials => "credentials",
            Disclosures => "disclosures",
            Issuances => "issuances",
            Jurisdiction => "jurisdiction",
            Country => "country",
            EntityType => "entity_type",
            Project => "project",
            Jurisdictions => "jurisdictions",
            EntityTypes => "entity_types",
            CustomText => "custom_text",
            CustomPhone => "custom_phone",
            CustomYesNo => "custom_yes_no",
            CustomSingleChoice => "custom_single_choice",
            CustomMultipleChoice => "custom_multiple_choice",
            CustomUsd => "custom_usd",
            CustomDatetime => "custom_datetime",
        }
    }

    /// Parse a `<type>` token into its variant, or `None` if it is not a
    /// registered type. This is the closed-set membership check `N113` runs.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        Self::iter().find(|t| t.as_str() == token)
    }

    /// Every registered `<type>` token — the canonical vocabulary the
    /// `rules` crate's `REGISTERED_QUESTION_TYPES` mirror is grounded to.
    #[must_use]
    pub fn all_tokens() -> Vec<&'static str> {
        Self::iter().map(|t| t.as_str()).collect()
    }

    /// Every aggregate (plural) `<type>` token — the tokens barred from the
    /// `__for_` child grammar. Grounds the `rules` mirror
    /// `AGGREGATE_QUESTION_TYPES`.
    #[must_use]
    pub fn aggregate_tokens() -> Vec<&'static str> {
        Self::iter()
            .filter(|t| t.cardinality() == Cardinality::Aggregate)
            .map(|t| t.as_str())
            .collect()
    }

    /// Parse the `<type>` out of a full `<type>__<role>` state name (or a
    /// bare `<type>`), then look it up. `__for_<role>` children resolve on
    /// their `<type>` prefix just like any other.
    #[must_use]
    pub fn from_state_name(state: &str) -> Option<Self> {
        let token = state.split("__").next().unwrap_or(state);
        Self::from_token(token)
    }

    /// Record, reference, or custom.
    #[must_use]
    pub fn kind(&self) -> Kind {
        use QuestionType::{
            Country, CustomDatetime, CustomMultipleChoice, CustomPhone, CustomSingleChoice,
            CustomText, CustomUsd, CustomYesNo, EntityType, EntityTypes, Jurisdiction,
            Jurisdictions, Project,
        };
        match self {
            CustomText | CustomPhone | CustomYesNo | CustomSingleChoice | CustomMultipleChoice
            | CustomUsd | CustomDatetime => Kind::Custom,
            Jurisdiction | Jurisdictions | Country | EntityType | EntityTypes | Project => {
                Kind::Reference
            }
            _ => Kind::Record,
        }
    }

    /// Singular (one row/value) or aggregate (an array of the singular's
    /// shape).
    #[must_use]
    pub fn cardinality(&self) -> Cardinality {
        use QuestionType::{
            Addresses, Credentials, Disclosures, Entities, EntityTypes, Filings, Issuances,
            Jurisdictions, People, Roles,
        };
        match self {
            People | Entities | Addresses | Roles | Filings | Credentials | Disclosures
            | Issuances | Jurisdictions | EntityTypes => Cardinality::Aggregate,
            _ => Cardinality::Singular,
        }
    }

    /// The aggregate form of a singular type, if one exists.
    #[must_use]
    pub fn plural(&self) -> Option<Self> {
        use QuestionType::{
            Address, Addresses, Credential, Credentials, Disclosure, Disclosures, Entities, Entity,
            EntityType, EntityTypes, Filing, Filings, Issuance, Issuances, Jurisdiction,
            Jurisdictions, People, Person, Role, Roles,
        };
        Some(match self {
            Person => People,
            Entity => Entities,
            Address => Addresses,
            Role => Roles,
            Filing => Filings,
            Credential => Credentials,
            Disclosure => Disclosures,
            Issuance => Issuances,
            Jurisdiction => Jurisdictions,
            EntityType => EntityTypes,
            _ => return None,
        })
    }

    /// The singular form of an aggregate type, if this is an aggregate.
    #[must_use]
    pub fn singular(&self) -> Option<Self> {
        use QuestionType::{
            Address, Addresses, Credential, Credentials, Disclosure, Disclosures, Entities, Entity,
            EntityType, EntityTypes, Filing, Filings, Issuance, Issuances, Jurisdiction,
            Jurisdictions, People, Person, Role, Roles,
        };
        Some(match self {
            People => Person,
            Entities => Entity,
            Addresses => Address,
            Roles => Role,
            Filings => Filing,
            Credentials => Credential,
            Disclosures => Disclosure,
            Issuances => Issuance,
            Jurisdictions => Jurisdiction,
            EntityTypes => EntityType,
            _ => return None,
        })
    }

    /// The table this type grounds to, or `None` for a custom primitive.
    /// Singular and aggregate forms ground to the same table (the aggregate
    /// is many rows of the singular).
    #[must_use]
    pub fn entity_table(&self) -> Option<&'static str> {
        use QuestionType::{
            Address, Addresses, Country, Credential, Credentials, Disclosure, Disclosures,
            Entities, Entity, EntityType, EntityTypes, Filing, Filings, Issuance, Issuances,
            Jurisdiction, Jurisdictions, Notarization, People, Person, Project, Role, Roles,
            Signature,
        };
        Some(match self {
            Person | People => "persons",
            Entity | Entities => "entities",
            Address | Addresses => "addresses",
            Role | Roles => "person_entity_roles",
            Filing | Filings => "filings",
            Credential | Credentials => "credentials",
            Disclosure | Disclosures => "disclosures",
            Issuance | Issuances => "share_issuances",
            Signature => "signatures",
            Notarization => "notarizations",
            Jurisdiction | Jurisdictions | Country => "jurisdictions",
            EntityType | EntityTypes => "entity_types",
            Project => "projects",
            _ => return None,
        })
    }

    /// The glossary term this type documents, for LSP hover and the docs
    /// grammar. `None` for custom primitives, which have no glossary entity.
    #[must_use]
    pub fn glossary_term(&self) -> Option<&'static str> {
        use QuestionType::{
            Address, Addresses, Country, Credential, Credentials, Disclosure, Disclosures,
            Entities, Entity, EntityType, EntityTypes, Filing, Filings, Issuance, Issuances,
            Jurisdiction, Jurisdictions, Notarization, People, Person, Project, Role, Roles,
            Signature,
        };
        Some(match self {
            Person | People => "Person",
            Entity | Entities => "Entity",
            Address | Addresses => "Address",
            Role | Roles => "Person Entity Role",
            Filing | Filings => "Filing",
            Credential | Credentials => "Credential",
            Disclosure | Disclosures => "Disclosure",
            Issuance | Issuances => "Share Issuance",
            Signature => "Signature",
            Notarization => "Notarization",
            Jurisdiction | Jurisdictions | Country => "Jurisdiction",
            EntityType | EntityTypes => "Entity Type",
            Project => "Project",
            _ => return None,
        })
    }

    /// The row parts an aggregate collects, or `&[]` for a singular. Today
    /// only `people` renders as a multi-part row widget; other aggregates
    /// collect a single reference per row.
    #[must_use]
    pub fn row_parts(&self) -> &'static [&'static str] {
        match self {
            QuestionType::People => &PERSON_ROW_PARTS,
            _ => &[],
        }
    }

    /// The `jurisdictions.jurisdiction_type` value this type's option
    /// list is restricted to, or `None` when the type doesn't select
    /// from the jurisdictions table (or offers all of it). The widget
    /// handlers read this instead of hardcoding the filter.
    #[must_use]
    pub fn jurisdiction_type_filter(&self) -> Option<&'static str> {
        match self {
            QuestionType::Country => Some("country"),
            _ => None,
        }
    }
}

/// The widget `answer_type` string that denotes an aggregate (plural)
/// question. Walkers and widgets dispatch on this rather than a hardcoded
/// `answer_type == "people_list"` special case: the `people_list` widget
/// collects the `people` aggregate.
#[must_use]
pub fn answer_type_is_aggregate(answer_type: &str) -> bool {
    answer_type == "people_list"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant round-trips through its token, and every token is
    /// distinct.
    ///
    /// This is the guard the `SeaORM` `DeriveActiveEnum` used to carry:
    /// the string form was declared twice — once in a `string_value`
    /// attribute and once in [`QuestionType::as_str`] — and the derive
    /// kept the pair honest. `as_str` is now the only declaration, so
    /// this pins it directly: a variant whose token collides with
    /// another's, or which `from_token` cannot resolve, fails here.
    /// `EnumIter` supplies the variant list, so a new variant is covered
    /// without editing this test.
    #[test]
    fn every_variant_round_trips_through_a_distinct_token() {
        use std::collections::BTreeSet;

        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        let mut count = 0;
        for qt in QuestionType::iter() {
            let token = qt.as_str();
            assert!(
                !token.is_empty(),
                "{qt:?} has an empty token — a state name cannot carry it"
            );
            assert!(
                seen.insert(token),
                "two question types share the token {token:?}; \
                 `from_token` would resolve it to whichever comes first"
            );
            assert_eq!(
                QuestionType::from_token(token),
                Some(qt),
                "{qt:?} does not round-trip through its own token {token:?}"
            );
            count += 1;
        }
        assert_eq!(
            seen.len(),
            count,
            "the registry lost a token to a collision"
        );
        assert_eq!(
            QuestionType::all_tokens().len(),
            count,
            "`all_tokens` must advertise exactly the registry"
        );
    }

    /// Every record/reference type declares the table its answers ground to,
    /// and the declared name must match the one spelled out here — a second,
    /// independent copy of the mapping, so a one-sided edit to either fails
    /// rather than silently redefining what a question type means.
    #[test]
    fn every_record_and_reference_type_grounds_to_a_real_table() {
        for qt in QuestionType::iter() {
            let real: Option<&'static str> = match qt {
                // `persons` is the one table here with no `store::entity`
                // module: it moved to SurrealDB with ENG-19, so its name is
                // named directly rather than read off an entity.
                QuestionType::Person | QuestionType::People => Some("persons"),
                // `entities` moved to SurrealDB with wave four
                // (ENG-120), so — like `persons` above — its name is
                // named directly rather than read off an entity.
                QuestionType::Entity | QuestionType::Entities => Some("entities"),
                // `addresses` moved to SurrealDB with the wave-two
                // ports, so — like `persons` above — its name is named
                // directly rather than read off an entity.
                QuestionType::Address | QuestionType::Addresses => Some("addresses"),
                // `person_entity_roles` moved with the same wave, as the
                // `entity_role` relation.
                QuestionType::Role | QuestionType::Roles => Some("person_entity_roles"),
                // `filings` moved to SurrealDB with wave five (ENG-121), so
                // — like `persons` above — its name is named directly
                // rather than read off an entity.
                QuestionType::Filing | QuestionType::Filings => Some("filings"),
                // `credentials` was retired with the
                // wave-two ports; licensure lives in `SurrealDB`, so —
                // like `persons` above — its name is named directly
                // rather than read off an entity.
                QuestionType::Credential | QuestionType::Credentials => Some("credentials"),
                // `disclosures` moved to SurrealDB with wave six (ENG-160),
                // so — like `persons` above — its name is named directly
                // rather than read off an entity.
                QuestionType::Disclosure | QuestionType::Disclosures => Some("disclosures"),
                // `share_issuances` was removed outright rather than ported:
                // the Firm keeps cap tables in Carta, so Navigator stores no
                // share ownership. The question type survives because a
                // questionnaire may still *ask* about an issuance — the
                // answer is recorded with the matter and reconciled against
                // Carta, not against a table here — so the name stays a
                // literal with nothing behind it to read it off.
                QuestionType::Issuance | QuestionType::Issuances => Some("share_issuances"),
                // `signatures` and `notarizations` moved to SurrealDB with
                // wave five (ENG-121), so — like `persons` above — their
                // names are named directly rather than read off an entity.
                QuestionType::Signature => Some("signatures"),
                QuestionType::Notarization => Some("notarizations"),
                // `jurisdictions` moved to SurrealDB with the ENG-20
                // slice, so — like `persons` above — its name is named
                // directly rather than read off an entity.
                QuestionType::Jurisdiction
                | QuestionType::Jurisdictions
                | QuestionType::Country => Some("jurisdictions"),
                // `entity_types` moved to SurrealDB with its wave-one
                // slice, so its name is also named directly.
                QuestionType::EntityType | QuestionType::EntityTypes => Some("entity_types"),
                // `project` is a SurrealDB cluster table, so it has no
                // SeaORM entity to supply its name.
                QuestionType::Project => Some("projects"),
                _ => None,
            };
            match qt.kind() {
                Kind::Custom => assert!(
                    qt.entity_table().is_none() && real.is_none(),
                    "{} is custom and must not ground to a table",
                    qt.as_str()
                ),
                Kind::Record | Kind::Reference => {
                    assert_eq!(
                        qt.entity_table(),
                        real,
                        "{} must ground to its declared table",
                        qt.as_str()
                    );
                }
            }
        }
    }

    /// No question type may point at a deny-listed table — the tables that
    /// are internal artifacts, comms, audit, billing, authz, or governance,
    /// not questionnaire vocabulary (issue #235's deny-list).
    #[test]
    fn no_type_grounds_to_a_deny_listed_table() {
        const DENY: &[&str] = &[
            "questions",
            "answers",
            "blobs",
            "templates",
            "notations",
            "communications",
            "sent_emails",
            "events",
            "git_access_tokens",
            "git_repositories",
            "invoices",
            "xero_invoice",
            "person_project_roles",
            "testimonials",
            "playbooks",
            "expunge_records",
            "expunge_requests",
            "relationship_edges",
            "letters",
            "mailroom",
            "attestations",
        ];
        for qt in QuestionType::iter() {
            if let Some(table) = qt.entity_table() {
                assert!(
                    !DENY.contains(&table),
                    "{} grounds to deny-listed table `{table}`",
                    qt.as_str()
                );
            }
        }
    }

    #[test]
    fn singular_and_plural_pair_symmetrically() {
        for qt in QuestionType::iter() {
            if let Some(plural) = qt.plural() {
                assert_eq!(qt.cardinality(), Cardinality::Singular);
                assert_eq!(plural.cardinality(), Cardinality::Aggregate);
                assert_eq!(
                    plural.singular(),
                    Some(qt),
                    "{} plural round-trips",
                    qt.as_str()
                );
            }
            if let Some(singular) = qt.singular() {
                assert_eq!(qt.cardinality(), Cardinality::Aggregate);
                assert_eq!(
                    singular.plural(),
                    Some(qt),
                    "{} singular round-trips",
                    qt.as_str()
                );
            }
        }
    }

    #[test]
    fn parses_type_out_of_state_names() {
        assert_eq!(
            QuestionType::from_state_name("entity__company"),
            Some(QuestionType::Entity)
        );
        assert_eq!(
            QuestionType::from_state_name("address__for_trustor"),
            Some(QuestionType::Address)
        );
        assert_eq!(
            QuestionType::from_state_name("custom_single_choice"),
            Some(QuestionType::CustomSingleChoice)
        );
        assert_eq!(QuestionType::from_state_name("not_a_type"), None);
    }

    #[test]
    fn people_row_parts_come_from_the_registry() {
        assert_eq!(QuestionType::People.row_parts(), &PERSON_ROW_PARTS);
        assert!(QuestionType::Person.row_parts().is_empty());
        assert!(answer_type_is_aggregate("people_list"));
        assert!(!answer_type_is_aggregate("string"));
    }
}
