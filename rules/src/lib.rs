//! Validation rules for markdown source files.
//!
//! Downstream consumers (the `cli` binary) build a
//! [`RuleEngine`] from a set of rules and run it over a directory.

pub mod c001;
pub mod c002;
pub mod c003;
pub mod citation;
pub mod d001;
pub mod d002;
pub mod d003;
pub mod d004;
pub mod dashboard;
pub mod e001;
pub mod e002;
pub mod e004;
pub mod engine;
pub mod f103;
pub mod f104;
pub mod f105;
pub mod f106;
pub mod f107;
pub mod f108;
pub mod f109;
pub mod f110;
pub mod f112;
pub mod f113;
pub mod f114;
pub mod f115;
pub mod f116;
pub mod f117;
pub mod f118;
pub mod f119;
pub mod frontmatter;
pub mod kind;
pub mod links;
pub mod m001;
pub mod m003;
pub mod m004;
pub mod m005;
pub mod m007;
pub mod m009;
pub mod m010;
pub mod m011;
pub mod m012;
pub mod m018;
pub mod m019;
pub mod m020;
pub mod m021;
pub mod m022;
pub mod m023;
pub mod m024;
pub mod m025;
pub mod m026;
pub mod m027;
pub mod m028;
pub mod m029;
pub mod m030;
pub mod m031;
pub mod m032;
pub mod m034;
pub mod m035;
pub mod m036;
pub mod m037;
pub mod m038;
pub mod m039;
pub mod m040;
pub mod m042;
pub mod m045;
pub mod m046;
pub mod m047;
pub mod m048;
pub mod m049;
pub mod m050;
pub mod m051;
pub mod m052;
pub mod m053;
pub mod m054;
pub mod m055;
pub mod m056;
pub mod m057;
pub mod m058;
pub mod m059;
pub mod m060;
pub mod m061;
pub mod questionnaire_hover;
pub mod s102;
pub mod s103;
pub mod s104;
pub mod workflow_steps;

pub use c001::C001ContentTitle;
pub use c002::C002ContentDescription;
pub use c003::C003BlogFilename;
pub use d001::D001UnknownSection;
pub use d002::D002OutOfCatalogSection;
pub use d003::D003RequiredSection;
pub use d004::D004LensComposition;
pub use dashboard::{Family, Lens, Section, Skeleton};
pub use e001::E001EventTimestamp;
pub use e002::E002EventTemplateExclusive;
pub use e004::E004EventLumaLink;
pub use f103::{is_pascal_case, is_snake_case, F103SnakeCaseFilename};
pub use f104::F104FlowQuestionCodes;
pub use f105::F105ConfidentialRequired;
pub use f106::F106LawyerReviewRequired;
pub use f107::F107SignaturePlaceholders;
pub use f108::F108TemplateCodeRequired;
pub use f109::F109OutputFormat;
pub use f110::{F110JurisdictionPath, JURISDICTIONS};
pub use f112::{workflow_step_not_built, F112WorkflowStepNotBuilt, WORKFLOW_STEPS_NOT_BUILT};
pub use f113::{
    bank_prompt, describe_question_type, F113TypeGrounding, BANK_PROMPTS, REGISTERED_QUESTION_TYPES,
};
pub use f114::{F114ForParentOrdering, AGGREGATE_QUESTION_TYPES};
pub use f115::F115PathResolution;
pub use f116::F116LawyerReviewGatesSubmission;
pub use f117::F117GlossaryBackedCustomText;
pub use f118::F118QuestionnaireLinearity;
pub use f119::{
    describe_change_surface, describe_github_notation, F119GithubNotation, CHANGE_SURFACES,
    CHANGE_SURFACE_STATE, ENGINEERING_COUNCIL_STATE, GITHUB_NOTATIONS, GITHUB_SHELF,
};
pub use kind::Kind;
pub use m001::M001HeadingIncrement;
pub use m003::M003HeadingStyle;
pub use m004::M004ULStyle;
pub use m005::M005ListIndent;
pub use m007::M007ULIndent;
pub use m009::M009NoTrailingSpaces;
pub use m010::M010NoHardTabs;
pub use m011::M011NoReversedLinks;
pub use m012::M012NoMultipleBlanks;
pub use m018::M018NoMissingSpaceATX;
pub use m019::M019NoMultipleSpaceATX;
pub use m020::M020NoMissingSpaceClosedATX;
pub use m021::M021NoMultipleSpaceClosedATX;
pub use m022::M022BlanksAroundHeadings;
pub use m023::M023HeadingStartLeft;
pub use m024::M024NoDuplicateHeading;
pub use m025::M025SingleH1;
pub use m026::M026NoTrailingPunctuation;
pub use m027::M027NoMultipleSpaceBlockquote;
pub use m028::M028NoBlanksBlockquote;
pub use m029::M029OLPrefix;
pub use m030::M030ListMarkerSpace;
pub use m031::M031BlanksAroundFences;
pub use m032::M032BlanksAroundLists;
pub use m034::M034NoBareUrls;
pub use m035::M035HRStyle;
pub use m036::M036NoEmphasisAsHeading;
pub use m037::M037NoSpaceInEmphasis;
pub use m038::M038NoSpaceInCode;
pub use m039::M039NoSpaceInLinks;
pub use m040::M040FencedCodeLanguage;
pub use m042::M042NoEmptyLinks;
pub use m045::M045NoAltText;
pub use m046::M046CodeBlockStyle;
pub use m047::M047SingleTrailingNewline;
pub use m048::M048CodeFenceStyle;
pub use m049::M049EmphasisStyle;
pub use m050::M050StrongStyle;
pub use m051::M051LinkFragments;
pub use m052::M052ReferenceLinksImages;
pub use m053::M053LinkImageReferenceDefinitions;
pub use m054::M054LinkImageStyle;
pub use m055::M055TablePipeStyle;
pub use m056::M056TableColumnCount;
pub use m057::M057RelativeLinkResolves;
pub use m058::M058BlanksAroundTables;
pub use m059::M059DescriptiveLinkText;
pub use m060::M060TableColumnStyle;
pub use m061::M061WebPortableLink;
pub use questionnaire_hover::{
    hover_markdown as questionnaire_prompt_hover, resolve_prompt, PromptProvenance, ResolvedPrompt,
};
pub use s102::S102LinePacking;
pub use s103::S103KindEnum;
pub use s104::S104MissingKind;
pub use workflow_steps::{
    is_allowed_prefix, lookup as lookup_workflow_step, StepStatus, WorkflowStep, WORKFLOW_STEPS,
};

pub use engine::{
    canonical_question_codes, classify_source, code_uniqueness_violations, lint_source_classified,
    navigator_classified_rules, navigator_classified_rules_with_codes, navigator_default_rules,
    navigator_default_rules_with_codes, navigator_event_rules, navigator_markdown_only_rules,
    ClassifiedRuleEngine, DefaultFileFilter, DocumentKind, FileFilter, LintReport, RuleEngine,
};

use std::ops::Range;
use std::path::PathBuf;

/// A source file presented to a rule for inspection.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub contents: String,
}

/// A single rule violation discovered during linting.
///
/// `range` is the byte offset span into the source file that the rule
/// is flagging — used by the LSP server and `cli validate --fix` to
/// pinpoint the edit. `line` is the 1-based line number derived from
/// `range.start`; kept as a separate field so the CLI text output and
/// existing consumers stay unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub code: &'static str,
    pub path: PathBuf,
    pub line: usize,
    pub range: Range<usize>,
    pub message: String,
}

/// The byte-offset range of a 1-based line within `contents`,
/// excluding the trailing line terminator (if any). Out-of-range lines
/// map to the end-of-file empty range, so callers can use this as a
/// safe default when the precise span isn't known.
///
/// The terminator is `\r\n` as readily as `\n`: a Windows checkout
/// materialises these files with CRLF, and a range that stopped only
/// before the `\n` would cover a trailing `\r` that is not part of the
/// line. That byte is load-bearing downstream — `M009` anchors its edit
/// to `range.end` and would delete the `\r` instead of the whitespace it
/// is fixing, and the LSP renders the range one column past the visible
/// end of line.
#[must_use]
pub fn line_byte_range(contents: &str, line: usize) -> Range<usize> {
    if line == 0 {
        return 0..0;
    }
    let mut offset = 0;
    for (idx, segment) in contents.split_inclusive('\n').enumerate() {
        if idx + 1 == line {
            let text = segment.strip_suffix('\n').unwrap_or(segment);
            let text = text.strip_suffix('\r').unwrap_or(text);
            return offset..offset + text.len();
        }
        offset += segment.len();
    }
    contents.len()..contents.len()
}

/// A single source-level edit that replaces `range` with `new_text`.
/// Mirrors the LSP `TextEdit` shape so the LSP server can pass these
/// straight through with only a byte-offset → UTF-16 position
/// conversion at the protocol boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

/// A validation rule that inspects a single file in isolation.
pub trait Rule: Send + Sync {
    fn code(&self) -> &'static str;
    fn lint(&self, file: &SourceFile) -> Vec<Violation>;

    /// One-line human-readable description of what this rule checks.
    /// Default returns a generic blurb; rules with a concise, useful
    /// summary should override this so editor tooltips and other
    /// surfaces can show why a violation matters at a glance.
    fn description(&self) -> &'static str {
        "Neon Law Navigator rule violation"
    }

    /// Produce a single source edit that resolves `violation`, if the
    /// rule is safe to autofix. Returns `None` by default — rules
    /// that flag prose / structural concerns (N-family, M024 dup
    /// headings, M026 trailing punctuation) intentionally stay
    /// diagnostic-only.
    fn fix(&self, _file: &SourceFile, _violation: &Violation) -> Option<TextEdit> {
        None
    }
}

/// Look up the one-line description for the given Neon Law Navigator rule
/// code. Returns the rule's `description()` if the code matches one
/// of the rules in [`navigator_default_rules`] (or `S102`, which is
/// markdown-only); falls back to a generic blurb otherwise.
///
/// This is the surface `navigator-lsp`'s hover tooltips read so
/// that hovering a violation explains the rule without re-walking
/// the registry on every lookup.
#[must_use]
pub fn description_for_code(code: &str) -> &'static str {
    match code {
        "S101" => "Line exceeds the 120-character limit",
        "S102" => "Line could absorb more text from the next line",
        "S103" => "Declared `kind:` must be a recognized document kind",
        "S104" => "A file's declared `kind:` must agree with its notation/event structure",
        "N101" => "Notation template must declare a non-empty `title`",
        "N102" => "Notation template must declare a valid `respondent_type`",
        "N103" => "Notation template filename must be snake_case",
        "N104" => "Notation questionnaire/workflow state references an unknown registry item",
        "N105" => "Notation template must declare `confidential`",
        "N106" => "Notation workflow must include lawyer review",
        "N107" => {
            "Signature placeholders must name a known signer/field and a signing workflow state"
        }
        "N108" => "Notation template must declare a stable `code`",
        "N109" => "Notation template `output:` must name a known render format",
        "N110" => "Notation template must live under neon_law/forms and declare jurisdiction",
        "N111" => "Notation template `code` must be unique across the tree",
        "N112" => "Workflow step is allowed but its automation is not built yet",
        "N113" => "Questionnaire state type must be a registered question type",
        "N114" => "`__for_` child state must follow a role-matched person/entity parent",
        "N115" => "Template data path or iterator must resolve against a typed questionnaire state",
        "N116" => "Notation workflow must gate every outbound submission behind lawyer review",
        "N117" => "every `custom_text__*` state must be an allowlisted free-text primitive",
        "N118" => "Questionnaire must be one linear `_` chain from BEGIN to END",
        "N119" => {
            "A `kind: github` notation must be `templates/github/create_issue.md` or \
             `create_pull_request.md` and ask the change-surface, Engineering Council, and \
             narrative questions"
        }
        "E001" => "Event must declare both a `starts_at` timestamp and a `timezone`",
        "E002" => "A file is either an event or a notation template, never both",
        "E004" => "Event must declare a `luma_url` to check it out on Luma",
        "C001" => "Content page must declare a non-empty `title`",
        "C002" => "Content page must declare a non-empty `description`",
        "C003" => "Blog post filename must be `YYYYMMDD_slug.md`",
        "D001" => "Matter dashboard section must be a recognized section type",
        "D002" => "Matter dashboard section must be in the declared kind's catalog",
        "D003" => "Matter dashboard must carry its required sections",
        "D004" => "Matter dashboard must declare a `lenses:` composition of known lenses",
        "M001" => "Heading levels must increment by one",
        "M003" => "Headings must use the ATX (`# Heading`) style",
        "M004" => "Unordered list markers must be consistent",
        "M005" => "List indentation must be consistent",
        "M007" => "Unordered list indentation must be a multiple of two",
        "M009" => "Lines must not end with trailing whitespace",
        "M010" => "Hard tabs are not allowed",
        "M011" => "Link syntax must be `[text](url)`, not the reverse",
        "M012" => "Multiple consecutive blank lines are not allowed",
        "M018" => "ATX headings must have a space after the `#`",
        "M019" => "ATX headings must have a single space after the `#`",
        "M020" => "Closed ATX headings must have a space before the closing `#`",
        "M021" => "Closed ATX headings must have a single space before the closing `#`",
        "M022" => "Headings must be surrounded by blank lines",
        "M023" => "Headings must start at column one",
        "M024" => "Headings must not duplicate prior siblings",
        "M025" => "A document must have at most one top-level (H1) heading",
        "M026" => "Headings must not end with punctuation",
        "M027" => "Blockquote markers must have a single space before content",
        "M028" => "Blockquotes must not contain blank lines",
        "M029" => "Ordered list items must use the configured prefix",
        "M030" => "List markers must have a single space before content",
        "M031" => "Fenced code blocks must be surrounded by blank lines",
        "M032" => "Lists must be surrounded by blank lines",
        "M034" => "Bare URLs must be wrapped in angle brackets",
        "M035" => "Horizontal rule style must be consistent",
        "M036" => "Emphasis must not be used in place of a heading",
        "M037" => "Emphasis markers must not have inner whitespace",
        "M038" => "Inline code spans must not have inner whitespace",
        "M039" => "Link text must not have inner whitespace",
        "M040" => "Fenced code blocks must declare a language",
        "M042" => "Links must not be empty",
        "M045" => "Images must declare alt text",
        "M046" => "Code block style must be consistent",
        "M047" => "File must end with a single trailing newline",
        "M048" => "Fenced code block markers must be consistent",
        "M049" => "Emphasis marker style must be consistent",
        "M050" => "Strong-emphasis marker style must be consistent",
        "M051" => "Link fragments must reference existing headings",
        "M052" => "Reference links must define their references",
        "M053" => "Reference definitions must be referenced",
        "M054" => "Link and image style must be consistent",
        "M055" => "Table pipe style must be consistent",
        "M056" => "Table column counts must match the header row",
        "M057" => "Relative link targets must resolve to a file on disk",
        "M058" => "Tables must be surrounded by blank lines",
        "M059" => "Link text must be descriptive (not `here`/`click`)",
        "M060" => "Table column styles must be consistent",
        "M061" => "Web-published docs must not link into non-rendered repo files",
        _ => "Neon Law Navigator rule violation",
    }
}

/// Whether a violation blocks the gate (`Error`) or merely advises
/// (`Warning`).
///
/// `navigator validate` and CI fail on `Error`-severity violations and
/// report `Warning`s without failing; `navigator-lsp` renders `Error`
/// as a red squiggle and `Warning` as a yellow one. Severity is keyed
/// off the rule code (a sibling of [`description_for_code`]) so the
/// existing `Violation` constructors across the rule modules stay
/// unchanged — adding a field would touch every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// A blocking problem: fails `navigator validate`, red in the editor.
    Error,
    /// A non-blocking advisory: reported but does not fail the gate,
    /// yellow in the editor. The "allowed but not built yet" signal.
    Warning,
}

/// The [`Severity`] for a Neon Law Navigator rule code.
///
/// Every rule is [`Severity::Error`] — a violation blocks the gate —
/// except the "not built yet" advisories (currently `N112`), which are
/// [`Severity::Warning`]: they surface as a yellow squiggle and are
/// reported by `navigator validate` without failing it. The `N112`
/// *rule* (the emitter) lands in a later change; its severity and
/// description are declared here first so the gate is severity-aware
/// before anything emits a warning.
#[must_use]
pub fn severity_for_code(code: &str) -> Severity {
    match code {
        // Two advisories that report without failing the gate: N112 is
        // the "allowed but not built yet" step warning, and M061 is
        // web-portability of docs links — the docs tree carries many
        // code-file links today, so it surfaces them for cleanup rather
        // than blocking. (M057, the disk-resolution check, stays an
        // error — a broken path is a bug.)
        "N112" | "M061" => Severity::Warning,
        _ => Severity::Error,
    }
}

/// `S101` — line length must not exceed `max` characters.
pub struct S101LineLength {
    pub max: usize,
}

impl S101LineLength {
    pub const CODE: &'static str = "S101";
    pub const DEFAULT_MAX: usize = 120;
}

impl Default for S101LineLength {
    fn default() -> Self {
        Self {
            max: Self::DEFAULT_MAX,
        }
    }
}

impl Rule for S101LineLength {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let max = self.max;
        // Skip fenced code blocks (triple-backtick / triple-tilde) —
        // long URLs, JSON, mermaid diagrams inside fences own their
        // own formatting and aren't reflowable. Frontmatter IS linted:
        // authors wrap long values across multiple lines using YAML
        // folded scalars (`description: >`), which `frontmatter::field`
        // collapses back to the same single-string value.
        let mut violations = Vec::new();
        let mut in_fence = false;
        for (idx, line) in file.contents.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            let count = line.chars().count();
            if count > max {
                violations.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line: idx + 1,
                    range: line_byte_range(&file.contents, idx + 1),
                    message: format!("Line is {count} characters (max {max})"),
                });
            }
        }
        violations
    }
}

/// `N101` — notation template must declare a non-empty `title` in YAML
/// frontmatter.
pub struct F101FrontmatterTitle;

impl F101FrontmatterTitle {
    pub const CODE: &'static str = "N101";
}

impl Rule for F101FrontmatterTitle {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let report = |message: &str| -> Vec<Violation> {
            vec![Violation {
                code: Self::CODE,
                path: file.path.clone(),
                line: 1,
                range: line_byte_range(&file.contents, 1),
                message: message.to_string(),
            }]
        };

        let Some(fm) = frontmatter::extract(&file.contents) else {
            return report("Missing YAML frontmatter (file must start with `---`)");
        };
        match frontmatter::field(fm, "title") {
            None => report("Frontmatter is missing required `title:` field"),
            Some(s) if s.is_empty() => report("Frontmatter `title:` is empty"),
            Some(_) => Vec::new(),
        }
    }
}

/// `N102` — notation template frontmatter must declare a
/// `respondent_type` whose value is one of `entity`, `person`, or
/// `person_and_entity`.
pub struct F102RespondentType;

impl F102RespondentType {
    pub const CODE: &'static str = "N102";
    pub const VALID: &[&'static str] = &["entity", "person", "person_and_entity"];
}

impl Rule for F102RespondentType {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let report = |message: String| -> Vec<Violation> {
            vec![Violation {
                code: Self::CODE,
                path: file.path.clone(),
                line: 1,
                range: line_byte_range(&file.contents, 1),
                message,
            }]
        };

        let Some(fm) = frontmatter::extract(&file.contents) else {
            return report("Missing YAML frontmatter (file must start with `---`)".to_string());
        };
        let value = match frontmatter::field(fm, "respondent_type") {
            None => {
                return report(format!(
                    "Frontmatter is missing required `respondent_type:` field (expected one of: {})",
                    Self::VALID.join(", ")
                ));
            }
            Some(s) if s.is_empty() => {
                return report(format!(
                    "Frontmatter `respondent_type:` is empty (expected one of: {})",
                    Self::VALID.join(", ")
                ));
            }
            Some(v) => v,
        };
        if Self::VALID.contains(&value.as_str()) {
            Vec::new()
        } else {
            report(format!(
                "Invalid `respondent_type:` value `{value}` (expected one of: {})",
                Self::VALID.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        description_for_code, F101FrontmatterTitle, F102RespondentType, Rule, S101LineLength,
        SourceFile,
    };
    use std::path::PathBuf;

    fn file(contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.md"),
            contents: contents.to_string(),
        }
    }

    #[test]
    fn s101_description_is_a_one_line_summary() {
        let s101 = S101LineLength::default();
        let description = s101.description();
        assert!(
            !description.is_empty() && description != s101.code(),
            "description should be a human summary, not the code"
        );
        assert!(
            !description.contains('\n'),
            "description must be a one-line string, got {description:?}"
        );
        // Stable shape: editor tooltips assume the summary names the
        // 120-character limit so users know what to fix.
        assert!(
            description.contains("120"),
            "S101's description should reference the 120-char limit, got {description:?}",
        );
    }

    #[test]
    fn description_for_code_covers_every_navigator_default_rule() {
        use super::{navigator_default_rules, navigator_markdown_only_rules};
        for rule in navigator_default_rules().iter().chain(
            navigator_markdown_only_rules()
                .iter()
                .filter(|r| r.code() == "S102"),
        ) {
            let description = description_for_code(rule.code());
            assert_ne!(
                description,
                "Neon Law Navigator rule violation",
                "{} has no custom description in description_for_code",
                rule.code(),
            );
        }
    }

    #[test]
    fn rule_codes_are_a_letter_prefix_then_digits_only() {
        // The rule-code grammar is a family letter (`M` for markdown,
        // `N`/`E`/`C`/`S`) followed by digits — never a trailing letter.
        // A `M057a`/`M057b`-style suffix is not a code; split such a rule
        // into two numbered codes (`M057`, `M061`) instead.
        use super::engine::{
            navigator_blog_rules, navigator_default_rules, navigator_event_rules,
            navigator_markdown_only_rules,
        };
        let mut codes: Vec<&'static str> = Vec::new();
        for set in [
            navigator_default_rules(),
            navigator_markdown_only_rules(),
            navigator_event_rules(),
            navigator_blog_rules(),
        ] {
            codes.extend(set.iter().map(|r| r.code()));
        }
        for code in codes {
            let mut chars = code.chars();
            let first = chars.next();
            assert!(
                first.is_some_and(|c| c.is_ascii_uppercase()),
                "rule code {code:?} must start with an uppercase family letter",
            );
            let rest: String = chars.collect();
            assert!(
                !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()),
                "rule code {code:?} must be a letter followed by digits only \
                 (no `a`/`b` suffix — use a new number instead)",
            );
        }
    }

    #[test]
    fn description_for_code_falls_back_for_unknown_code() {
        assert_eq!(
            description_for_code("XYZ"),
            "Neon Law Navigator rule violation"
        );
    }

    #[test]
    fn severity_defaults_to_error() {
        use super::{severity_for_code, Severity};
        // Every shipped rule blocks the gate by default.
        assert_eq!(severity_for_code("N104"), Severity::Error);
        assert_eq!(severity_for_code("M010"), Severity::Error);
        assert_eq!(severity_for_code("S101"), Severity::Error);
        // Unknown codes fall back to Error (fail closed).
        assert_eq!(severity_for_code("XYZ"), Severity::Error);
    }

    #[test]
    fn not_built_advisory_is_a_warning() {
        use super::{severity_for_code, Severity};
        // N112 is the "allowed but not built yet" advisory: yellow, not
        // blocking. Its emitting rule lands later; the severity is
        // declared here so the gate is severity-aware first.
        assert_eq!(severity_for_code("N112"), Severity::Warning);
    }

    #[test]
    fn n112_has_a_real_description() {
        assert_ne!(
            description_for_code("N112"),
            "Neon Law Navigator rule violation",
            "N112 must carry a human description for editor tooltips",
        );
    }

    #[test]
    fn line_byte_range_finds_line_offsets() {
        use super::line_byte_range;
        let contents = "first\nsecond\nthird\n";
        assert_eq!(line_byte_range(contents, 1), 0..5);
        assert_eq!(line_byte_range(contents, 2), 6..12);
        assert_eq!(line_byte_range(contents, 3), 13..18);
    }

    #[test]
    fn line_byte_range_handles_missing_trailing_newline() {
        use super::line_byte_range;
        let contents = "only-line";
        assert_eq!(line_byte_range(contents, 1), 0..9);
    }

    #[test]
    fn line_byte_range_returns_eof_for_out_of_range_lines() {
        use super::line_byte_range;
        let contents = "a\nb\n";
        let eof = contents.len()..contents.len();
        assert_eq!(line_byte_range(contents, 99), eof);
        assert_eq!(line_byte_range(contents, 0), 0..0);
    }

    /// The range must cover the line, not the line plus its `\r`. A
    /// Windows checkout materialises these files with CRLF, and the
    /// surplus byte is not cosmetic: `M009` anchors its edit to
    /// `range.end`, so a range holding the `\r` made its fix delete the
    /// carriage return instead of the trailing whitespace.
    #[test]
    fn line_byte_range_excludes_a_crlf_carriage_return() {
        use super::line_byte_range;
        let contents = "hello\r\nworld\r\n";
        assert_eq!(line_byte_range(contents, 1), 0..5);
        assert_eq!(&contents[line_byte_range(contents, 1)], "hello");
        assert_eq!(line_byte_range(contents, 2), 7..12);
        assert_eq!(&contents[line_byte_range(contents, 2)], "world");
    }

    /// Every line of a CRLF document spans the same text as the
    /// matching line of its LF twin.
    #[test]
    fn line_byte_range_matches_across_lf_and_crlf_twins() {
        use super::line_byte_range;
        let lf = "first\nsecond\nthird\n";
        let crlf = lf.replace('\n', "\r\n");
        for line in 1..=3 {
            assert_eq!(
                &lf[line_byte_range(lf, line)],
                &crlf[line_byte_range(&crlf, line)],
                "line {line} differs between the twins"
            );
        }
    }

    #[test]
    fn rule_fix_default_returns_none() {
        use super::{Rule, SourceFile, Violation};
        struct Diagnostic;
        impl Rule for Diagnostic {
            fn code(&self) -> &'static str {
                "DIAG"
            }
            fn lint(&self, _file: &SourceFile) -> Vec<Violation> {
                Vec::new()
            }
        }
        let file = SourceFile {
            path: std::path::PathBuf::from("t.md"),
            contents: String::new(),
        };
        let v = Violation {
            code: "DIAG",
            path: file.path.clone(),
            line: 1,
            range: 0..0,
            message: String::new(),
        };
        assert!(Diagnostic.fix(&file, &v).is_none());
    }

    #[test]
    fn rule_trait_default_description_is_non_empty() {
        // Sanity: rules that don't override description still return a
        // non-empty string — so editor tooltips never render a blank
        // bubble even for rules we haven't customized yet.
        struct Dummy;
        impl Rule for Dummy {
            fn code(&self) -> &'static str {
                "DUMMY"
            }
            fn lint(&self, _file: &SourceFile) -> Vec<super::Violation> {
                Vec::new()
            }
        }
        let d = Dummy;
        assert!(!d.description().is_empty());
    }

    #[test]
    fn s101_accepts_line_exactly_at_limit() {
        let rule = S101LineLength::default();
        let line = "x".repeat(S101LineLength::DEFAULT_MAX);
        assert!(rule.lint(&file(&line)).is_empty());
    }

    #[test]
    fn s101_flags_line_one_over_limit() {
        let rule = S101LineLength::default();
        let line = "x".repeat(S101LineLength::DEFAULT_MAX + 1);
        let violations = rule.lint(&file(&line));
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, "S101");
        assert_eq!(violations[0].line, 1);
    }

    #[test]
    fn s101_reports_correct_line_numbers_for_mixed_input() {
        let rule = S101LineLength::default();
        let long = "x".repeat(121);
        let contents = format!("short\n{long}\nshort\n{long}");
        let violations = rule.lint(&file(&contents));
        assert_eq!(
            violations.iter().map(|v| v.line).collect::<Vec<_>>(),
            vec![2, 4]
        );
    }

    #[test]
    fn s101_counts_unicode_scalars_not_bytes() {
        // 121 Greek letters: 121 chars, but 242 bytes (each is 2 bytes in UTF-8).
        // The rule must count chars, so 121 should violate (over 120).
        let rule = S101LineLength::default();
        let line = "α".repeat(121);
        assert_eq!(rule.lint(&file(&line)).len(), 1);

        // 120 of the same — exactly at the limit — should not violate.
        let line = "α".repeat(120);
        assert!(rule.lint(&file(&line)).is_empty());
    }

    #[test]
    fn s101_respects_configurable_max() {
        let rule = S101LineLength { max: 80 };
        let line = "x".repeat(81);
        assert_eq!(rule.lint(&file(&line)).len(), 1);
    }

    #[test]
    fn s101_reports_its_code() {
        assert_eq!(S101LineLength::default().code(), "S101");
    }

    #[test]
    fn s101_flags_long_lines_inside_yaml_frontmatter() {
        let rule = S101LineLength::default();
        let long = "x".repeat(200);
        let contents = format!("---\ndescription: {long}\n---\n\nshort body\n");
        let violations = rule.lint(&file(&contents));
        assert_eq!(violations.len(), 1, "frontmatter lines must trip S101");
        // Line 2 is the long `description:` line.
        assert_eq!(violations[0].line, 2);
    }

    #[test]
    fn s101_passes_folded_scalar_whose_continuation_lines_fit() {
        // The same long value reflowed through a `>` block scalar —
        // every physical line is under 120, so S101 is silent. The
        // YAML parse value is identical (one folded string).
        let rule = S101LineLength::default();
        let contents = "---\n\
                        description: >\n\
                          A long description that has been wrapped\n\
                          across two indented lines under a folded\n\
                          block scalar marker.\n\
                        ---\n";
        assert!(rule.lint(&file(contents)).is_empty());
    }

    #[test]
    fn s101_still_flags_long_body_lines_when_frontmatter_present() {
        let rule = S101LineLength::default();
        let long = "x".repeat(200);
        let contents = format!("---\ntitle: ok\n---\n\n{long}\n");
        let violations = rule.lint(&file(&contents));
        assert_eq!(violations.len(), 1);
        // Line 5 is the long body line (after `---`, `title: ok`, `---`, blank).
        assert_eq!(violations[0].line, 5);
    }

    #[test]
    fn s101_does_not_treat_a_lone_dashed_line_as_frontmatter() {
        // Without an opening `---` on line 1, a `---` separator
        // mid-document must not silence the rule.
        let rule = S101LineLength::default();
        let long = "x".repeat(200);
        let contents = format!("# Heading\n\n---\n{long}\n");
        assert_eq!(rule.lint(&file(&contents)).len(), 1);
    }

    #[test]
    fn f101_passes_when_title_present() {
        let contents = "---\ntitle: My Document\n---\n\nBody text";
        assert!(F101FrontmatterTitle.lint(&file(contents)).is_empty());
    }

    #[test]
    fn f101_flags_missing_frontmatter() {
        let violations = F101FrontmatterTitle.lint(&file("# Just a heading\nNo frontmatter"));
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, "N101");
        assert!(violations[0].message.contains("Missing"));
    }

    #[test]
    fn f101_flags_missing_title_field() {
        let contents = "---\nrespondent_type: entity\n---\n";
        let violations = F101FrontmatterTitle.lint(&file(contents));
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("missing"));
    }

    #[test]
    fn f101_flags_empty_title_value() {
        let contents = "---\ntitle:\n---\n";
        let violations = F101FrontmatterTitle.lint(&file(contents));
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("empty"));
    }

    #[test]
    fn f101_passes_with_other_fields_present() {
        let contents = "---\nrespondent_type: entity\ntitle: Trust\nauthor: Jane\n---\n";
        assert!(F101FrontmatterTitle.lint(&file(contents)).is_empty());
    }

    #[test]
    fn f101_passes_when_frontmatter_has_no_trailing_newline() {
        let contents = "---\ntitle: Inline\n---";
        assert!(F101FrontmatterTitle.lint(&file(contents)).is_empty());
    }

    #[test]
    fn f102_accepts_each_valid_respondent_type() {
        for value in F102RespondentType::VALID {
            let contents = format!("---\nrespondent_type: {value}\n---\n");
            assert!(
                F102RespondentType.lint(&file(&contents)).is_empty(),
                "expected {value} to be accepted",
            );
        }
    }

    #[test]
    fn f102_flags_invalid_value() {
        let contents = "---\nrespondent_type: corporation\n---\n";
        let violations = F102RespondentType.lint(&file(contents));
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].code, "N102");
        assert!(violations[0].message.contains("corporation"));
    }

    #[test]
    fn f102_flags_missing_field() {
        let contents = "---\ntitle: Trust\n---\n";
        let violations = F102RespondentType.lint(&file(contents));
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("missing"));
    }

    #[test]
    fn f102_flags_empty_value() {
        let contents = "---\nrespondent_type:\n---\n";
        let violations = F102RespondentType.lint(&file(contents));
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("empty"));
    }

    #[test]
    fn f102_flags_missing_frontmatter() {
        let violations = F102RespondentType.lint(&file("no frontmatter here"));
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("Missing"));
    }
}
