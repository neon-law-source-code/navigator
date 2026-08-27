//! Directory walking, file filtering, and rule orchestration.
//!
//! The engine reads markdown files under a directory, applies every
//! configured rule to each one, and returns the aggregated violations.
//! Non-markdown files, hidden directories (anything starting with `.`), and
//! `target/` are skipped by default; every other `.md` is linted, and
//! `classify_source` decides which rule set each one earns.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use walkdir::WalkDir;

use crate::{Rule, SourceFile, Violation};

/// The result of linting a directory: how many files were inspected
/// and every violation produced.
#[derive(Debug, Default)]
pub struct LintReport {
    pub files_scanned: usize,
    pub violations: Vec<Violation>,
}

/// The validation family a Markdown file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    /// Ordinary prose/content Markdown: READMEs, docs, blog posts,
    /// marketing pages, and other files whose frontmatter is not the
    /// Neon Law Navigator notation contract.
    Markdown,
    /// A Neon Law Navigator notation Template: the static blueprint that declares
    /// a questionnaire/workflow and becomes a running Notation later.
    NotationTemplate,
    /// A dated public event: markdown under
    /// `web/content/events/` whose frontmatter declares a `starts_at`
    /// timestamp. An event never declares a questionnaire/workflow, and a
    /// notation template never declares a timestamp — see [`E002`].
    ///
    /// [`E002`]: crate::E002EventTemplateExclusive
    Event,
    /// A published blog post under `web/content/blog/`: dated markdown
    /// (`YYYYMMDD_slug.md`) carrying a `title` and `description`. Not a
    /// notation template — it gets the prose rules plus the content-page
    /// frontmatter rules (`C001`/`C002`) and the dated-filename rule
    /// (`C003`).
    BlogPost,
    /// A public workshop / teaching page under `web/content/workshops/`,
    /// carrying a `title` and `description`. Gets the prose rules plus the
    /// shared content-page rules (`C001`/`C002`).
    Workshop,
    /// A GitHub intake notation under `templates/github/`: the
    /// questionnaire that gathers what an issue or pull request needs, and
    /// the body that renders it. It carries a questionnaire like a notation
    /// template but is not a legal instrument, so it gets the
    /// questionnaire-grammar rules plus `N119` and none of the legal
    /// contract — see [`navigator_github_rules`].
    Github,
    /// An authored **matter dashboard** composition (#896): a page type an
    /// attorney composes from registered sections, declaring its per-lens
    /// section lists in frontmatter. It carries no questionnaire and
    /// becomes no instrument, so it gets the prose rules plus the D-family
    /// composition rules — see [`navigator_dashboard_rules`].
    MatterDashboard,
}

impl LintReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// True when at least one violation is [`crate::Severity::Error`].
    ///
    /// `navigator validate` fails the gate on this rather than on the
    /// mere presence of any violation, so [`crate::Severity::Warning`]
    /// advisories (e.g. "step not built yet") are reported without
    /// failing the build.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.violations
            .iter()
            .any(|v| crate::severity_for_code(v.code) == crate::Severity::Error)
    }
}

/// Decides whether a directory or file should be visited.
pub trait FileFilter: Send + Sync {
    fn include_dir(&self, path: &Path) -> bool;
    fn include_file(&self, path: &Path) -> bool;
}

/// The default filter: skip hidden directories (`.git`, `.build`,
/// `.claude`, …) and `target/`, and lint every other `*.md` file.
///
/// Classification (`classify_source`) is what now decides a file's rule
/// set — a file with no notation frontmatter classifies as prose
/// [`DocumentKind::Markdown`] and is held only to the Markdown rules — so
/// there is no default name/directory allowlist: `validate` sees the whole
/// tree. `excluded_names` and `excluded_directories` remain as data (empty
/// by default) so a caller can still skip a vendored or generated subtree.
#[derive(Default)]
pub struct DefaultFileFilter {
    pub excluded_names: Vec<String>,
    pub excluded_directories: Vec<String>,
}

impl FileFilter for DefaultFileFilter {
    fn include_dir(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return true;
        };
        if name.starts_with('.') {
            return false;
        }
        if name == "target" {
            return false;
        }
        !self.excluded_directories.iter().any(|n| n == name)
    }

    fn include_file(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        let is_md = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_md {
            return false;
        }
        if self.excluded_names.iter().any(|n| n == name) {
            return false;
        }
        // Reject if any ancestor directory matches an excluded name.
        for ancestor in path.components() {
            if let std::path::Component::Normal(seg) = ancestor {
                if let Some(s) = seg.to_str() {
                    if self.excluded_directories.iter().any(|n| n == s) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// Orchestrates a set of [`Rule`]s over a directory of markdown files.
pub struct RuleEngine {
    rules: Vec<Box<dyn Rule>>,
    filter: Box<dyn FileFilter>,
}

impl RuleEngine {
    #[must_use]
    pub fn new(rules: Vec<Box<dyn Rule>>) -> Self {
        Self {
            rules,
            filter: Box::new(DefaultFileFilter::default()),
        }
    }

    #[must_use]
    pub fn with_filter(mut self, filter: Box<dyn FileFilter>) -> Self {
        self.filter = filter;
        self
    }

    /// Walk `dir`, lint every included markdown file, and return the
    /// aggregated report. Returns an `io::Error` if the directory
    /// can't be read or a file fails to load.
    pub fn lint_directory(&self, dir: &Path) -> io::Result<LintReport> {
        let mut report = LintReport::default();
        for entry in WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() && e.depth() > 0 {
                    self.filter.include_dir(e.path())
                } else {
                    true
                }
            })
        {
            let entry = entry.map_err(io::Error::other)?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !self.filter.include_file(path) {
                continue;
            }
            let contents = fs::read_to_string(path)?;
            let file = SourceFile {
                path: PathBuf::from(path),
                contents,
            };
            for rule in &self.rules {
                report.violations.extend(rule.lint(&file));
            }
            report.files_scanned += 1;
        }
        Ok(report)
    }
}

/// A rule engine that chooses the rule set per file.
///
/// Prose Markdown gets [`navigator_markdown_only_rules`]; notation
/// templates get [`navigator_default_rules`]. This is the workspace-wide
/// mode for mixed trees where marketing/blog/docs files and notation
/// templates can all carry YAML frontmatter without sharing the same
/// semantic contract.
pub struct ClassifiedRuleEngine {
    filter: Box<dyn FileFilter>,
    valid_question_codes: Vec<String>,
}

impl ClassifiedRuleEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            filter: Box::new(DefaultFileFilter::default()),
            valid_question_codes: canonical_question_codes(),
        }
    }

    #[must_use]
    pub fn with_question_codes(mut self, codes: Vec<String>) -> Self {
        self.valid_question_codes = codes;
        self
    }

    #[must_use]
    pub fn with_filter(mut self, filter: Box<dyn FileFilter>) -> Self {
        self.filter = filter;
        self
    }

    /// Walk `dir`, classify every included markdown file, and lint it
    /// with the matching Neon Law Navigator rule set.
    pub fn lint_directory(&self, dir: &Path) -> io::Result<LintReport> {
        let mut report = LintReport::default();
        for entry in WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() && e.depth() > 0 {
                    self.filter.include_dir(e.path())
                } else {
                    true
                }
            })
        {
            let entry = entry.map_err(io::Error::other)?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !self.filter.include_file(path) {
                continue;
            }
            let contents = fs::read_to_string(path)?;
            let file = SourceFile {
                path: PathBuf::from(path),
                contents,
            };
            report.violations.extend(lint_source_classified_with_codes(
                &file,
                &self.valid_question_codes,
            ));
            report.files_scanned += 1;
        }
        Ok(report)
    }
}

impl Default for ClassifiedRuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Cross-file check (`N111`): every notation template's frontmatter
/// `code` — the questionnaire/workflow key that uniquely identifies a
/// template — must be unique across `dir`. A duplicate `code` means two
/// templates claim the same identity, which breaks import and the
/// per-Project archive key, so it is reported as a violation on the
/// second (and later) file in sorted-path order. This is a directory
/// pass, not a per-file [`Rule`], because uniqueness is only visible
/// across the whole tree.
pub fn code_uniqueness_violations(
    dir: &Path,
    filter: &dyn FileFilter,
) -> io::Result<Vec<Violation>> {
    use std::collections::HashMap;

    // Collect (path, code) for every classified NotationTemplate, then
    // sort by path so "first declaration" is deterministic regardless of
    // filesystem walk order.
    let mut entries: Vec<(PathBuf, String, String)> = Vec::new();
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() && e.depth() > 0 {
                filter.include_dir(e.path())
            } else {
                true
            }
        })
    {
        let entry = entry.map_err(io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !filter.include_file(path) {
            continue;
        }
        let contents = fs::read_to_string(path)?;
        let file = SourceFile {
            path: PathBuf::from(path),
            contents,
        };
        if classify_source(&file) != DocumentKind::NotationTemplate {
            continue;
        }
        let Some(fm) = crate::frontmatter::extract(&file.contents) else {
            continue;
        };
        match crate::frontmatter::field(fm, "code") {
            Some(code) if !code.is_empty() => entries.push((file.path, code, file.contents)),
            _ => {}
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut first_seen: HashMap<String, PathBuf> = HashMap::new();
    let mut violations = Vec::new();
    for (path, code, contents) in entries {
        if let Some(prev) = first_seen.get(&code) {
            violations.push(Violation {
                code: "N111",
                path: path.clone(),
                line: 1,
                range: crate::line_byte_range(&contents, 1),
                message: format!(
                    "Duplicate template `code` `{code}`; already declared in `{}`",
                    prev.display()
                ),
            });
        } else {
            first_seen.insert(code, path);
        }
    }
    Ok(violations)
}

#[derive(Debug, Deserialize)]
struct CanonicalQuestions {
    records: Vec<CanonicalQuestion>,
}

#[derive(Debug, Deserialize)]
struct CanonicalQuestion {
    code: String,
}

/// The canonical seeded question-code registry, bundled from
/// `store/seeds/Question.yaml` so the CLI, LSP, and CI all validate
/// notation templates against the same list the workspace seeds.
#[must_use]
pub fn canonical_question_codes() -> Vec<String> {
    const QUESTION_YAML: &str = include_str!("../../store/seeds/Question.yaml");
    serde_yaml::from_str::<CanonicalQuestions>(QUESTION_YAML)
        .expect("embedded store/seeds/Question.yaml must deserialize for N104 validation")
        .records
        .into_iter()
        .map(|q| q.code)
        .collect()
}

/// The canonical Neon Law Navigator rule set, in the stable presentation
/// order. `N104` validates questionnaire states against the canonical
/// `store/seeds/Question.yaml` question-code list by default.
#[must_use]
pub fn navigator_default_rules() -> Vec<Box<dyn Rule>> {
    use crate::{
        E002EventTemplateExclusive, F101FrontmatterTitle, F102RespondentType,
        F103SnakeCaseFilename, F104FlowQuestionCodes, F105ConfidentialRequired,
        F106LawyerReviewRequired, F107SignaturePlaceholders, F108TemplateCodeRequired,
        F109OutputFormat, F110JurisdictionPath, F112WorkflowStepNotBuilt, F113TypeGrounding,
        F114ForParentOrdering, F115PathResolution, F116LawyerReviewGatesSubmission,
        F117GlossaryBackedCustomText, F118QuestionnaireLinearity, F120BodyStateGrounding,
        M001HeadingIncrement, M003HeadingStyle, M004ULStyle, M005ListIndent, M007ULIndent,
        M009NoTrailingSpaces, M010NoHardTabs, M011NoReversedLinks, M012NoMultipleBlanks,
        M018NoMissingSpaceATX, M019NoMultipleSpaceATX, M020NoMissingSpaceClosedATX,
        M021NoMultipleSpaceClosedATX, M022BlanksAroundHeadings, M023HeadingStartLeft,
        M024NoDuplicateHeading, M025SingleH1, M026NoTrailingPunctuation,
        M027NoMultipleSpaceBlockquote, M028NoBlanksBlockquote, M029OLPrefix, M030ListMarkerSpace,
        M031BlanksAroundFences, M032BlanksAroundLists, M034NoBareUrls, M035HRStyle,
        M037NoSpaceInEmphasis, M038NoSpaceInCode, M039NoSpaceInLinks, M040FencedCodeLanguage,
        M042NoEmptyLinks, M045NoAltText, M046CodeBlockStyle, M047SingleTrailingNewline,
        M048CodeFenceStyle, M049EmphasisStyle, M050StrongStyle, M051LinkFragments,
        M052ReferenceLinksImages, M053LinkImageReferenceDefinitions, M054LinkImageStyle,
        M055TablePipeStyle, M056TableColumnCount, M057RelativeLinkResolves, M058BlanksAroundTables,
        M059DescriptiveLinkText, M060TableColumnStyle, M061WebPortableLink, S101LineLength,
        S103KindEnum, S104MissingKind,
    };
    vec![
        Box::new(S101LineLength::default()),
        // S103/S104 are cross-cutting (they validate the `kind:`
        // discriminator on every file), so they live in the default set
        // and survive the markdown-only filter, which only drops the N and
        // E families. S103 checks a declared value; S104 checks that a file
        // carrying notation/event structure declared one at all.
        Box::new(S103KindEnum),
        Box::new(S104MissingKind),
        Box::new(F101FrontmatterTitle),
        Box::new(F102RespondentType),
        Box::new(F103SnakeCaseFilename),
        Box::new(F104FlowQuestionCodes::new(canonical_question_codes())),
        Box::new(F105ConfidentialRequired),
        Box::new(F106LawyerReviewRequired),
        Box::new(F107SignaturePlaceholders),
        Box::new(F108TemplateCodeRequired),
        Box::new(F109OutputFormat),
        Box::new(F110JurisdictionPath),
        Box::new(F112WorkflowStepNotBuilt),
        Box::new(F113TypeGrounding),
        Box::new(F114ForParentOrdering),
        Box::new(F115PathResolution),
        Box::new(F116LawyerReviewGatesSubmission),
        Box::new(F117GlossaryBackedCustomText),
        Box::new(F118QuestionnaireLinearity),
        Box::new(F120BodyStateGrounding),
        // Mutual exclusivity runs on templates too: a template that wrongly
        // declares a `starts_at` timestamp is flagged here (the event side
        // is enforced by `navigator_event_rules`).
        Box::new(E002EventTemplateExclusive),
        Box::new(M001HeadingIncrement),
        Box::new(M003HeadingStyle),
        Box::new(M004ULStyle),
        Box::new(M005ListIndent),
        Box::new(M007ULIndent),
        Box::new(M009NoTrailingSpaces),
        Box::new(M010NoHardTabs),
        Box::new(M011NoReversedLinks),
        Box::new(M012NoMultipleBlanks),
        Box::new(M018NoMissingSpaceATX),
        Box::new(M019NoMultipleSpaceATX),
        Box::new(M020NoMissingSpaceClosedATX),
        Box::new(M021NoMultipleSpaceClosedATX),
        Box::new(M022BlanksAroundHeadings),
        Box::new(M023HeadingStartLeft),
        Box::new(M024NoDuplicateHeading),
        Box::new(M025SingleH1),
        Box::new(M026NoTrailingPunctuation),
        Box::new(M027NoMultipleSpaceBlockquote),
        Box::new(M028NoBlanksBlockquote),
        Box::new(M029OLPrefix),
        Box::new(M030ListMarkerSpace),
        Box::new(M031BlanksAroundFences),
        Box::new(M032BlanksAroundLists),
        Box::new(M034NoBareUrls),
        Box::new(M035HRStyle),
        Box::new(M037NoSpaceInEmphasis),
        Box::new(M038NoSpaceInCode),
        Box::new(M039NoSpaceInLinks),
        Box::new(M040FencedCodeLanguage),
        Box::new(M042NoEmptyLinks),
        Box::new(M045NoAltText),
        Box::new(M046CodeBlockStyle),
        Box::new(M047SingleTrailingNewline),
        Box::new(M048CodeFenceStyle),
        Box::new(M049EmphasisStyle),
        Box::new(M050StrongStyle),
        Box::new(M051LinkFragments),
        Box::new(M052ReferenceLinksImages),
        Box::new(M053LinkImageReferenceDefinitions),
        Box::new(M054LinkImageStyle),
        Box::new(M055TablePipeStyle),
        Box::new(M056TableColumnCount),
        Box::new(M057RelativeLinkResolves),
        Box::new(M058BlanksAroundTables),
        Box::new(M059DescriptiveLinkText),
        Box::new(M060TableColumnStyle),
        Box::new(M061WebPortableLink),
    ]
}

/// The default notation-template rules with strict question-code
/// validation enabled by a caller-supplied registry. No database is
/// touched here; callers decide whether and how to load the codes.
#[must_use]
pub fn navigator_default_rules_with_codes(valid_codes: &[String]) -> Vec<Box<dyn Rule>> {
    let mut rules = navigator_default_rules();
    for rule in &mut rules {
        if rule.code() == "N104" {
            *rule = Box::new(crate::F104FlowQuestionCodes::new(
                valid_codes.iter().cloned(),
            ));
        }
    }
    rules
}

/// The Markdown-only subset of [`navigator_default_rules`] — every
/// rule except the N-family, plus `S102` (line-packing). Suitable for
/// linting arbitrary prose markdown (READMEs, blog posts, marketing
/// copy) that doesn't carry the Neon Law Navigator notation frontmatter and
/// that benefits from being packed tight to the 120-character budget.
///
/// `S102` is markdown-only rather than universal because template
/// fixtures intentionally keep some lines short for readability
/// alongside their structured YAML; only free-form prose should be
/// reflowed to the limit.
#[must_use]
pub fn navigator_markdown_only_rules() -> Vec<Box<dyn Rule>> {
    // Drop both the N-family (notation) and the E-family (event) rules:
    // plain prose is neither a template nor an event. The event rules are
    // re-added explicitly by `navigator_event_rules`.
    let mut rules: Vec<Box<dyn Rule>> = navigator_default_rules()
        .into_iter()
        .filter(|r| !r.code().starts_with('N') && !r.code().starts_with('E'))
        .collect();
    // M036 (emphasis-as-heading) is prose-only, not in the default
    // (notation-template) set: legal template bodies legitimately set
    // standalone bold labels in signature blocks (`**Employee**`), which
    // are not headings. Slot it in canonical order, right after M035.
    let insert_m036 = rules
        .iter()
        .position(|r| r.code() == "M035")
        .map_or(rules.len(), |i| i + 1);
    rules.insert(insert_m036, Box::new(crate::M036NoEmphasisAsHeading));
    // Place S102 right after S101 so the two line-length rules sit
    // next to each other.
    let insert_at = rules
        .iter()
        .position(|r| r.code() == "S101")
        .map_or(0, |i| i + 1);
    rules.insert(insert_at, Box::new(crate::S102LinePacking::default()));
    rules
}

/// The rule set for a dated public event markdown file.
///
/// Events get the prose Markdown rules (so headings, links, and the
/// 120-character budget are still enforced on the body), the shared
/// content-page rules (`C001`/`C002` — an event page needs a `title` and
/// a `description` the same way a blog post does), plus the E-family
/// event-contract rules. They deliberately do *not* get the N-family
/// notation rules — an event is not a template.
#[must_use]
pub fn navigator_event_rules() -> Vec<Box<dyn Rule>> {
    use crate::{E001EventTimestamp, E002EventTemplateExclusive, E004EventLumaLink};
    let mut rules = navigator_content_page_rules();
    rules.push(Box::new(E001EventTimestamp));
    rules.push(Box::new(E002EventTemplateExclusive));
    rules.push(Box::new(E004EventLumaLink));
    rules
}

/// Prose Markdown rules plus the shared content-page frontmatter rules
/// (`C001` title, `C002` description). This is the base for every
/// published `web/content` page that is not a notation template —
/// events, blog posts, and workshops — each of which then adds its
/// own kind-specific rule(s).
#[must_use]
fn navigator_content_page_rules() -> Vec<Box<dyn Rule>> {
    use crate::{C001ContentTitle, C002ContentDescription};
    let mut rules = navigator_markdown_only_rules();
    rules.push(Box::new(C001ContentTitle));
    rules.push(Box::new(C002ContentDescription));
    rules
}

/// The rule set for a blog post under `web/content/blog/`: the shared
/// content-page rules plus `C003`, which pins the `YYYYMMDD_slug.md`
/// filename the loader silently depends on.
#[must_use]
pub fn navigator_blog_rules() -> Vec<Box<dyn Rule>> {
    let mut rules = navigator_content_page_rules();
    rules.push(Box::new(crate::C003BlogFilename));
    rules
}

/// The rule set for a public workshop page under
/// `web/content/workshops/`: the shared content-page rules (a workshop
/// page needs a `title` and `description` like any other content page).
/// It carries no kind-specific filename rule — workshop materials are
/// named by topic, not a dated/quarterly convention.
#[must_use]
pub fn navigator_workshop_rules() -> Vec<Box<dyn Rule>> {
    navigator_content_page_rules()
}

/// The N-family rules a `kind: github` notation is held to.
///
/// A GitHub notation borrows the questionnaire grammar, so every rule that
/// checks *that grammar* applies: the states are typed (`N113`), the chain
/// is linear (`N118`), the body's placeholders resolve against declared
/// states (`N115` for the dotted paths, `N120` for the bare typed tokens),
/// and every custom question is defined (`N104`, in questionnaire-only mode
/// — a GitHub notation renders a body and stops, so it declares no
/// `workflow:`).
///
/// Everything the legal contract adds is deliberately absent, because a
/// GitHub notation has nothing for those rules to check: no respondent
/// (`N102`), no confidentiality classification (`N105`), no lawyer review or
/// outbound submission to gate (`N106`, `N116`), no signature block
/// (`N107`), no render format or catalog code (`N108`, `N109`, `N111`), no
/// jurisdiction or legal shelf (`N110`), and no `__for_` role nesting
/// (`N114`). `N117`'s free-text allowlist is legal-glossary reasoning — it
/// exists so client facts get typed states instead of prose — and a GitHub
/// notation holds no client facts at all, only engineering narrative, so it
/// is not applied here.
const GITHUB_NOTATION_RULE_CODES: &[&str] =
    &["N101", "N103", "N104", "N113", "N115", "N118", "N120"];

/// The rule set for a GitHub intake notation under `templates/github/`.
///
/// Built from the notation set rather than the prose set on purpose: it
/// keeps `S101`'s hard 120-character limit but not `S102`'s greedy reflow,
/// which fights a file whose prose sits alongside structured YAML.
#[must_use]
pub fn navigator_github_rules() -> Vec<Box<dyn Rule>> {
    navigator_github_rules_with_codes(&canonical_question_codes())
}

/// [`navigator_github_rules`] with a caller-supplied question-code registry.
#[must_use]
pub fn navigator_github_rules_with_codes(valid_codes: &[String]) -> Vec<Box<dyn Rule>> {
    let mut rules: Vec<Box<dyn Rule>> = navigator_default_rules()
        .into_iter()
        .filter(|rule| {
            let code = rule.code();
            let family = code.starts_with('N') || code.starts_with('E');
            !family || GITHUB_NOTATION_RULE_CODES.contains(&code)
        })
        .collect();
    for rule in &mut rules {
        if rule.code() == "N104" {
            *rule = Box::new(crate::F104FlowQuestionCodes::questionnaire_only(
                valid_codes.iter().cloned(),
            ));
        }
    }
    rules.push(Box::new(crate::F119GithubNotation));
    rules
}

/// Classify a source file before choosing its validation rule set.
///
/// Classification reads the declared `kind:` and **only** that — it does
/// not infer the family from a `questionnaire:`/`workflow:` block, a
/// `starts_at:` timestamp, or the file's path. A declared `kind:` names
/// the family outright; a file that declares none (or an unrecognized
/// value) is plain prose [`DocumentKind::Markdown`]. [`crate::S103KindEnum`]
/// flags a bad value and [`crate::S104MissingKind`] flags a file that
/// carries notation/event structure yet forgot to declare a kind, so a
/// real template can never silently classify as prose.
#[must_use]
pub fn classify_source(file: &SourceFile) -> DocumentKind {
    let Some(kind) = crate::kind::declared(&file.contents) else {
        return DocumentKind::Markdown;
    };
    if kind.is_notation() {
        return DocumentKind::NotationTemplate;
    }
    if kind.is_dashboard() {
        return DocumentKind::MatterDashboard;
    }
    match kind {
        crate::kind::Kind::Event => DocumentKind::Event,
        crate::kind::Kind::Post => DocumentKind::BlogPost,
        crate::kind::Kind::Workshop => DocumentKind::Workshop,
        crate::kind::Kind::Github => DocumentKind::Github,
        // Every notation-family kind is handled by the `is_notation()`
        // branch above; the content kinds are exhausted here.
        _ => DocumentKind::NotationTemplate,
    }
}

/// The rule set for an authored matter dashboard composition (#896): the
/// prose rules every Markdown file earns, plus the D-family rules that
/// check the composition against the declared kind's skeleton.
///
/// Deliberately not the notation rules — a dashboard declares no
/// questionnaire, no jurisdiction, and no confidentiality classification,
/// so holding it to the legal N-family would report nothing but noise.
#[must_use]
pub fn navigator_dashboard_rules() -> Vec<Box<dyn Rule>> {
    let mut rules = navigator_markdown_only_rules();
    rules.push(Box::new(crate::D001UnknownSection));
    rules.push(Box::new(crate::D002OutOfCatalogSection));
    rules.push(Box::new(crate::D003RequiredSection));
    rules.push(Box::new(crate::D004LensComposition));
    rules
}

#[must_use]
pub fn navigator_classified_rules(file: &SourceFile) -> Vec<Box<dyn Rule>> {
    navigator_classified_rules_with_codes(file, &canonical_question_codes())
}

#[must_use]
pub fn navigator_classified_rules_with_codes(
    file: &SourceFile,
    valid_codes: &[String],
) -> Vec<Box<dyn Rule>> {
    match classify_source(file) {
        DocumentKind::Markdown => navigator_markdown_only_rules(),
        DocumentKind::NotationTemplate => navigator_default_rules_with_codes(valid_codes),
        DocumentKind::Event => navigator_event_rules(),
        DocumentKind::BlogPost => navigator_blog_rules(),
        DocumentKind::Workshop => navigator_workshop_rules(),
        DocumentKind::Github => navigator_github_rules_with_codes(valid_codes),
        DocumentKind::MatterDashboard => navigator_dashboard_rules(),
    }
}

#[must_use]
pub fn lint_source_classified(file: &SourceFile) -> Vec<Violation> {
    lint_source_classified_with_codes(file, &canonical_question_codes())
}

fn lint_source_classified_with_codes(file: &SourceFile, valid_codes: &[String]) -> Vec<Violation> {
    let rule_set = navigator_classified_rules_with_codes(file, valid_codes);
    rule_set.iter().flat_map(|r| r.lint(file)).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_question_codes, classify_source, lint_source_classified, navigator_default_rules,
        ClassifiedRuleEngine, DefaultFileFilter, DocumentKind, FileFilter, RuleEngine,
    };
    use crate::{F101FrontmatterTitle, F102RespondentType, Rule, S101LineLength};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// Minimal three-rule set used by the engine integration tests
    /// that assert specific violation counts. The full default rule
    /// set is exercised by the parity test below.
    fn minimal_engine_rules() -> Vec<Box<dyn Rule>> {
        vec![
            Box::new(S101LineLength::default()),
            Box::new(F101FrontmatterTitle),
            Box::new(F102RespondentType),
        ]
    }

    fn source(path: &str, contents: &str) -> crate::SourceFile {
        crate::SourceFile {
            path: PathBuf::from(path),
            contents: contents.to_string(),
        }
    }

    #[test]
    fn default_filter_includes_every_markdown_file() {
        // Classification chooses the rule set after the default filter
        // presents every Markdown file, including README and CLAUDE.
        let f = DefaultFileFilter::default();
        assert!(f.include_file(Path::new("foo/bar.md")));
        assert!(f.include_file(Path::new("foo/README.md")));
        assert!(f.include_file(Path::new("foo/CLAUDE.md")));
        assert!(!f.include_file(Path::new("foo/notes.txt")));
    }

    #[test]
    fn filter_honors_explicit_excludes_as_data() {
        // The name/directory allowlists still work when a caller sets them;
        // they are just empty by default.
        let f = DefaultFileFilter {
            excluded_names: vec!["SKIP.md".to_string()],
            excluded_directories: vec!["vendored".to_string()],
        };
        assert!(!f.include_file(Path::new("foo/SKIP.md")));
        assert!(!f.include_file(Path::new("vendored/thing.md")));
        assert!(!f.include_dir(Path::new("foo/vendored")));
        assert!(f.include_file(Path::new("foo/keep.md")));
    }

    #[test]
    fn default_filter_skips_hidden_dirs_and_target() {
        let f = DefaultFileFilter::default();
        assert!(!f.include_dir(Path::new("foo/.git")));
        assert!(!f.include_dir(Path::new("foo/.claude")));
        assert!(!f.include_dir(Path::new("foo/target")));
        assert!(f.include_dir(Path::new("foo/src")));
    }

    #[test]
    fn engine_returns_empty_report_for_directory_with_no_markdown() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "notes.txt", "not markdown");
        let report = RuleEngine::new(minimal_engine_rules())
            .lint_directory(dir.path())
            .unwrap();
        assert_eq!(report.files_scanned, 0);
        assert!(report.is_clean());
    }

    #[test]
    fn has_errors_ignores_warning_severity_violations() {
        // A report carrying only a Warning-severity advisory (N112) is
        // not clean, but must not fail the gate.
        let report = super::LintReport {
            files_scanned: 1,
            violations: vec![crate::Violation {
                code: "N112",
                path: PathBuf::from("trust.md"),
                line: 1,
                range: 0..0,
                message: "step not built yet".to_string(),
            }],
        };
        assert!(
            !report.is_clean(),
            "a warning is still a reported violation"
        );
        assert!(!report.has_errors(), "a warning must not fail the gate");
    }

    #[test]
    fn has_errors_is_true_when_an_error_violation_is_present() {
        let report = super::LintReport {
            files_scanned: 1,
            violations: vec![crate::Violation {
                code: "N104",
                path: PathBuf::from("trust.md"),
                line: 1,
                range: 0..0,
                message: "unknown step".to_string(),
            }],
        };
        assert!(report.has_errors());
    }

    #[test]
    fn engine_lints_valid_file_with_no_violations() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "trust.md",
            "---\ntitle: Trust\nrespondent_type: entity\n---\n\nBody.",
        );
        let report = RuleEngine::new(minimal_engine_rules())
            .lint_directory(dir.path())
            .unwrap();
        assert_eq!(report.files_scanned, 1);
        assert!(report.is_clean(), "{:?}", report.violations);
    }

    #[test]
    fn engine_aggregates_violations_across_rules_and_files() {
        let dir = TempDir::new().unwrap();
        // File 1: line too long AND missing respondent_type.
        write(
            dir.path(),
            "a.md",
            &format!("---\ntitle: A\n---\n\n{}", "x".repeat(121)),
        );
        // File 2: missing title.
        write(
            dir.path(),
            "sub/b.md",
            "---\nrespondent_type: person\n---\n",
        );
        // File 3: valid — should produce no violations.
        write(
            dir.path(),
            "c.md",
            "---\ntitle: C\nrespondent_type: person_and_entity\n---\n",
        );
        let report = RuleEngine::new(minimal_engine_rules())
            .lint_directory(dir.path())
            .unwrap();
        assert_eq!(report.files_scanned, 3);
        let codes: Vec<&str> = report.violations.iter().map(|v| v.code).collect();
        assert!(codes.contains(&"S101"));
        assert!(codes.contains(&"N101"));
        assert!(codes.contains(&"N102"));
        // No false positives from c.md.
        assert_eq!(report.violations.len(), 3);
    }

    /// Stable presentation order of the default rule set. Embedded
    /// literally so this test fails loudly if a future change
    /// silently reorders or drops a rule.
    const EXPECTED_DEFAULT_RULE_CODES: &[&str] = &[
        "S101", "S103", "S104", "N101", "N102", "N103", "N104", "N105", "N106", "N107", "N108",
        "N109", "N110", "N112", "N113", "N114", "N115", "N116", "N117", "N118", "N120", "E002",
        "M001", "M003", "M004", "M005", "M007", "M009", "M010", "M011", "M012", "M018", "M019",
        "M020", "M021", "M022", "M023", "M024", "M025", "M026", "M027", "M028", "M029", "M030",
        "M031", "M032", "M034", "M035", "M037", "M038", "M039", "M040", "M042", "M045", "M046",
        "M047", "M048", "M049", "M050", "M051", "M052", "M053", "M054", "M055", "M056", "M057",
        "M058", "M059", "M060", "M061",
    ];

    #[test]
    fn navigator_default_rule_codes_are_stable() {
        let actual_codes: Vec<&'static str> =
            navigator_default_rules().iter().map(|r| r.code()).collect();
        assert_eq!(
            actual_codes, EXPECTED_DEFAULT_RULE_CODES,
            "default rule set order drifted; update EXPECTED_DEFAULT_RULE_CODES intentionally if this was on purpose"
        );
    }

    #[test]
    fn canonical_question_codes_include_custom_text() {
        let codes = canonical_question_codes();
        assert!(
            codes.iter().any(|code| code == "custom_text"),
            "canonical question registry should include the reusable custom_text prompt code"
        );
    }

    #[test]
    fn navigator_markdown_only_rules_drop_n_family_and_add_s102_and_m036() {
        use super::navigator_markdown_only_rules;
        let codes: Vec<&'static str> = navigator_markdown_only_rules()
            .iter()
            .map(|r| r.code())
            .collect();
        assert!(codes.iter().all(|c| !c.starts_with('N')));
        // The markdown-only set drops the event family too.
        assert!(codes.iter().all(|c| !c.starts_with('E')));
        let mut expected: Vec<&str> = EXPECTED_DEFAULT_RULE_CODES
            .iter()
            .copied()
            .filter(|c| !c.starts_with('N') && !c.starts_with('E'))
            .collect();
        // M036 (emphasis-as-heading) is prose-only — added to this set
        // rather than the default one — and sits in canonical order,
        // right after M035.
        let m036_pos = expected.iter().position(|c| *c == "M035").unwrap() + 1;
        expected.insert(m036_pos, "M036");
        // S102 sits right after S101.
        let s102_pos = expected.iter().position(|c| *c == "S101").unwrap() + 1;
        expected.insert(s102_pos, "S102");
        assert_eq!(codes, expected);
    }

    /// The GitHub set is the questionnaire-grammar subset plus `N119` — no
    /// more, no less. Pinned as an exact list so adding a legal rule to the
    /// default set cannot silently start applying it to a shelf that has
    /// nothing for it to check.
    #[test]
    fn navigator_github_rules_keep_the_questionnaire_subset_and_add_n119() {
        use super::{navigator_github_rules, GITHUB_NOTATION_RULE_CODES};
        let codes: Vec<&'static str> = navigator_github_rules().iter().map(|r| r.code()).collect();

        let n_codes: Vec<&str> = codes
            .iter()
            .copied()
            .filter(|c| c.starts_with('N'))
            .collect();
        let mut expected: Vec<&str> = GITHUB_NOTATION_RULE_CODES.to_vec();
        expected.push("N119");
        assert_eq!(n_codes, expected);

        // The legal contract is absent: no respondent, confidentiality,
        // lawyer review, signature, catalog code, jurisdiction, or the
        // free-text allowlist that exists to keep client facts typed.
        for absent in [
            "N102", "N105", "N106", "N107", "N108", "N109", "N110", "N112", "N114", "N116", "N117",
        ] {
            assert!(
                !codes.contains(&absent),
                "{absent} is legal-contract only and must not run on a GitHub notation",
            );
        }
        // The event family never applies, and the prose-only reflow rule
        // (S102) stays off a file whose text sits beside structured YAML.
        assert!(codes.iter().all(|c| !c.starts_with('E')));
        assert!(!codes.contains(&"S102"));
        // The cross-cutting kind checks and the hard line limit still run.
        for present in ["S101", "S103", "S104"] {
            assert!(codes.contains(&present), "{present} must still run");
        }
    }

    /// A GitHub notation classifies from its declared kind, and the
    /// classifier hands it the GitHub rule set.
    #[test]
    fn a_github_notation_classifies_and_gets_its_own_rule_set() {
        let file = source(
            "templates/github/create_issue.md",
            "---\nkind: github\ntitle: T\n---\n\nBody.\n",
        );
        assert_eq!(classify_source(&file), DocumentKind::Github);
        let codes: Vec<&'static str> = super::navigator_classified_rules(&file)
            .iter()
            .map(|r| r.code())
            .collect();
        assert!(codes.contains(&"N119"), "got {codes:?}");
        assert!(!codes.contains(&"N110"), "got {codes:?}");
    }

    /// `N104` in questionnaire-only mode still validates the questionnaire —
    /// it only stops demanding a `workflow:` a GitHub notation would never
    /// run. The two halves are asserted together so the relaxation cannot
    /// quietly widen into "skip the questionnaire too".
    #[test]
    fn questionnaire_only_n104_drops_the_workflow_requirement_and_nothing_else() {
        use crate::{F104FlowQuestionCodes, Rule};
        let no_workflow = source(
            "templates/github/create_issue.md",
            "---\nkind: github\ntitle: T\nquestionnaire:\n  BEGIN:\n    _: custom_text__x\n  \
             custom_text__x:\n    _: END\n  END: {}\ncustom_questions:\n  x:\n    prompt: P\n---\n\nBody.\n",
        );
        let codes = canonical_question_codes();
        assert!(
            F104FlowQuestionCodes::questionnaire_only(codes.clone())
                .lint(&no_workflow)
                .is_empty(),
            "a GitHub notation needs no workflow",
        );
        assert!(
            F104FlowQuestionCodes::new(codes.clone())
                .lint(&no_workflow)
                .iter()
                .any(|v| v.message.contains("Missing required `workflow` key")),
            "the legal mode still demands one",
        );

        // The questionnaire half is untouched: an undefined custom question
        // is still flagged in questionnaire-only mode.
        let undefined = source(
            "templates/github/create_issue.md",
            "---\nkind: github\ntitle: T\nquestionnaire:\n  BEGIN:\n    _: custom_text__x\n  \
             custom_text__x:\n    _: END\n  END: {}\n---\n\nBody.\n",
        );
        assert!(
            F104FlowQuestionCodes::questionnaire_only(codes)
                .lint(&undefined)
                .iter()
                .any(|v| v.message.contains("custom_questions")),
            "questionnaire validation must still run",
        );
    }

    #[test]
    fn classifier_treats_code_alone_as_markdown() {
        let file = source(
            "web/content/marketing/service.md",
            "---\ntitle: Service\ncode: northstar\n---\n\nBody.\n",
        );
        assert_eq!(classify_source(&file), DocumentKind::Markdown);
    }

    #[test]
    fn publish_frontmatter_does_not_change_markdown_classification_or_validation() {
        let file = source(
            "docs/public-guide.md",
            "---\npublish: true\n---\n\n# Public guide\n",
        );
        assert_eq!(classify_source(&file), DocumentKind::Markdown);
        assert!(
            lint_source_classified(&file).is_empty(),
            "the docs publication flag is not a rules frontmatter key"
        );
    }

    #[test]
    fn structure_without_a_declared_kind_is_plain_markdown() {
        // Classification no longer infers from structure: a file that
        // declares a questionnaire/workflow but no `kind:` is plain
        // Markdown (S104 flags the omission separately — see its tests).
        let questionnaire = source(
            "draft.md",
            "---\ntitle: Draft\nquestionnaire:\n  BEGIN:\n    _: END\n---\n",
        );
        let workflow = source(
            "draft.md",
            "---\ntitle: Draft\nworkflow:\n  BEGIN:\n    created: END\n---\n",
        );
        assert_eq!(classify_source(&questionnaire), DocumentKind::Markdown);
        assert_eq!(classify_source(&workflow), DocumentKind::Markdown);
    }

    #[test]
    fn a_declared_kind_names_the_family_directly() {
        // Every notation-family kind classifies as a notation template,
        // regardless of path or whether a questionnaire block is present.
        for value in [
            "letter",
            "filing",
            "will",
            "trust",
            "directive",
            "agreement",
            "onboarding",
            "offboarding",
            "memo",
        ] {
            let file = source(
                "anywhere.md",
                &format!("---\ntitle: T\nkind: {value}\n---\n"),
            );
            assert_eq!(
                classify_source(&file),
                DocumentKind::NotationTemplate,
                "kind `{value}` should classify as a notation template",
            );
        }
        // Content kinds map to their family regardless of path.
        let event = source("anywhere.md", "---\ntitle: T\nkind: event\n---\n");
        assert_eq!(classify_source(&event), DocumentKind::Event);
        let post = source("anywhere.md", "---\ntitle: T\nkind: post\n---\n");
        assert_eq!(classify_source(&post), DocumentKind::BlogPost);
        let workshop = source("anywhere.md", "---\ntitle: T\nkind: workshop\n---\n");
        assert_eq!(classify_source(&workshop), DocumentKind::Workshop);
    }

    #[test]
    fn unrecognized_kind_classifies_as_markdown() {
        // A bad `kind:` value names no family, so the file is plain
        // Markdown (S103 reports the bad value separately).
        let file = source(
            "draft.md",
            "---\ntitle: T\nkind: bogus\nworkflow:\n  BEGIN:\n    created: END\n---\n",
        );
        assert_eq!(classify_source(&file), DocumentKind::Markdown);
    }

    #[test]
    fn a_templates_path_alone_does_not_classify() {
        // Classification is declaration-driven, not path-driven: a file
        // under `templates/` with no `kind:` is plain Markdown.
        let plain = source("templates/trust/draft.md", "Plain body.\n");
        assert_eq!(classify_source(&plain), DocumentKind::Markdown);
        let titled = source("templates/trust/draft.md", "---\ntitle: T\n---\n\nBody.\n");
        assert_eq!(classify_source(&titled), DocumentKind::Markdown);
    }

    #[test]
    fn a_content_path_alone_does_not_classify() {
        // Path no longer classifies content pages either — a blog or
        // workshop file that forgets its `kind:` is plain Markdown (the
        // corpus guard test keeps the real content roots honest).
        let blog = source(
            "web/content/blog/20260625_going_all_in_on_rust.md",
            "---\ntitle: Going All-In on Rust\ndescription: Why.\n---\n\nBody.\n",
        );
        assert_eq!(classify_source(&blog), DocumentKind::Markdown);
        let workshop = source(
            "server/content/workshops/navigator/DEPLOY.md",
            "---\ntitle: Deploy\ndescription: How.\n---\n\nBody.\n",
        );
        assert_eq!(classify_source(&workshop), DocumentKind::Markdown);
    }

    #[test]
    fn blog_post_requires_title_description_and_dated_filename() {
        // A misnamed, under-specified post trips all three content rules:
        // missing title (C001), missing description (C002), bad filename
        // (C003) — none of which the loader would report; it would just
        // drop the post.
        let file = source(
            "web/content/blog/draft.md",
            "---\nkind: post\nslug: x\n---\n\nBody.\n",
        );
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"C001"), "expected C001, got {codes:?}");
        assert!(codes.contains(&"C002"), "expected C002, got {codes:?}");
        assert!(codes.contains(&"C003"), "expected C003, got {codes:?}");
        assert!(
            !codes.iter().any(|c| c.starts_with('N')),
            "a blog post is not a notation template: {codes:?}"
        );
    }

    #[test]
    fn plain_prose_never_gets_content_page_rules() {
        // A README-style prose doc has no title/description requirement —
        // the C-family must not leak onto ordinary Markdown.
        let file = source("docs/some-guide.md", "# Guide\n\nProse body.\n");
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(
            !codes.iter().any(|c| c.starts_with('C')),
            "prose markdown must not trip C-family rules: {codes:?}"
        );
    }

    #[test]
    fn template_with_only_workflow_is_flagged_for_missing_questionnaire() {
        // The "both or neither" invariant: declaring one machine
        // classifies the file as a template (||), and N104 then requires
        // the other. A half-declared template never lints clean.
        let workflow_only = source(
            "templates/neon_law/shared/draft.md",
            "---\nkind: onboarding\ntitle: Draft\ncode: x__draft\nworkflow:\n  BEGIN:\n    created: END\n---\n",
        );
        let codes: Vec<&str> = lint_source_classified(&workflow_only)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"N104"), "expected N104, got {codes:?}");
        assert!(
            lint_source_classified(&workflow_only)
                .iter()
                .any(|v| v.code == "N104" && v.message.contains("questionnaire")),
            "N104 should name the missing questionnaire"
        );

        let questionnaire_only = source(
            "templates/neon_law/shared/draft.md",
            "---\nkind: onboarding\ntitle: Draft\ncode: x__draft\nquestionnaire:\n  BEGIN:\n    _: END\n---\n",
        );
        assert!(
            lint_source_classified(&questionnaire_only)
                .iter()
                .any(|v| v.code == "N104" && v.message.contains("workflow")),
            "N104 should name the missing workflow"
        );
    }

    #[test]
    fn declared_event_with_a_questionnaire_trips_e002() {
        // A file declared `kind: event` classifies as an Event; if it also
        // (wrongly) declares a questionnaire, E002 flags the conflict.
        let file = source(
            "web/content/events/20260702_bad.md",
            "---\nkind: event\nstarts_at: \"2026-07-02T11:00:00\"\nquestionnaire:\n  BEGIN:\n    _: END\n---\n",
        );
        assert_eq!(classify_source(&file), DocumentKind::Event);
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"E002"), "expected E002, got {codes:?}");
    }

    #[test]
    fn event_rules_apply_e_family_not_n_family() {
        // An event missing its timezone trips E001 but never an N-rule.
        let file = source(
            "web/content/events/20260702_x.md",
            "---\nkind: event\ntitle: X\nstarts_at: \"2026-07-02T11:00:00\"\nluma_url: https://luma.com/x\n---\n\nBody.\n",
        );
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"E001"), "expected E001, got {codes:?}");
        assert!(codes.iter().all(|c| !c.starts_with('N')));
    }

    #[test]
    fn template_with_timestamp_trips_e002() {
        let file = source(
            "templates/trust/draft.md",
            "---\nkind: filing\ntitle: T\ncode: x\nworkflow:\n  BEGIN:\n    a: END\nstarts_at: \"2026-07-02T11:00:00\"\n---\n",
        );
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"E002"), "expected E002, got {codes:?}");
    }

    #[test]
    fn classified_lint_does_not_apply_n_rules_to_code_only_content() {
        let file = source(
            "web/content/marketing/service.md",
            "---\ntitle: Service\ncode: northstar\n---\n\nBody.\n",
        );
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(
            codes.iter().all(|code| !code.starts_with('N')),
            "code-only content frontmatter must stay prose markdown, got {codes:?}",
        );
    }

    #[test]
    fn classified_lint_requires_code_for_notation_template() {
        let file = source(
            "draft.md",
            r"---
kind: onboarding
title: Draft
respondent_type: person
confidential: true
questionnaire:
  BEGIN:
    created: person__client
  person__client:
    answered: END
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
---
Body.
",
        );
        let codes: Vec<&str> = lint_source_classified(&file)
            .iter()
            .map(|v| v.code)
            .collect();
        assert!(
            codes.contains(&"N108"),
            "expected missing code violation, got {codes:?}"
        );
    }

    #[test]
    fn classified_lint_rejects_unknown_question_code_from_canonical_registry() {
        let file = source(
            "templates/custom/draft.md",
            r"---
kind: onboarding
title: Draft
respondent_type: person
code: draft__unknown_question
confidential: true
questionnaire:
  BEGIN:
    _: not_a_seeded_question
  not_a_seeded_question:
    _: END
workflow:
  BEGIN:
    _: lawyer_review
  lawyer_review:
    _: END
---
Body.
",
        );
        let violations = lint_source_classified(&file);
        assert!(
            violations
                .iter()
                .any(|v| v.code == "N104" && v.message.contains("not_a_seeded_question")),
            "expected N104 for unknown canonical question code, got {violations:?}"
        );
    }

    #[test]
    fn classified_engine_lints_a_mixed_tree_from_the_filesystem_alone() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "web/content/marketing/service.md",
            "---\ntitle: Service\ncode: northstar\n---\n\nBody.\n",
        );
        write(
            dir.path(),
            "templates/neon_law/northstar/nv__generic_trust.md",
            r"---
kind: trust
title: Nevada Trust
respondent_type: entity
code: trusts__nevada
jurisdiction: NV
confidential: true
questionnaire:
  BEGIN:
    created: person__client
  person__client:
    answered: END
workflow:
  BEGIN:
    created: lawyer_review
  lawyer_review:
    approved: END
---
Body.
",
        );
        let report = ClassifiedRuleEngine::new()
            .lint_directory(dir.path())
            .unwrap();
        assert_eq!(report.files_scanned, 2);
        // The trust template's `lawyer_review` gate earns a yellow N112
        // "not built yet" advisory, so the report is not strictly clean —
        // but it carries no blocking errors.
        assert!(!report.has_errors(), "{:?}", report.violations);
        assert!(
            report.violations.iter().all(|v| v.code == "N112"),
            "only the not-built advisory is expected, got {:?}",
            report.violations,
        );
    }

    #[test]
    fn code_uniqueness_flags_duplicate_codes() {
        use super::code_uniqueness_violations;
        let dir = TempDir::new().unwrap();
        let tmpl = |code: &str| {
            format!(
                "---\nkind: trust\ntitle: T\nquestionnaire:\n  BEGIN:\n    _: END\ncode: {code}\n---\nBody.\n"
            )
        };
        // Two templates share `trusts__nevada`; one is unique.
        write(dir.path(), "templates/a/x.md", &tmpl("trusts__nevada"));
        write(dir.path(), "templates/b/y.md", &tmpl("trusts__nevada"));
        write(dir.path(), "templates/c/z.md", &tmpl("wills__simple"));
        let v = code_uniqueness_violations(dir.path(), &DefaultFileFilter::default()).unwrap();
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N111");
        // The second file in sorted order (b/y.md) is the one flagged.
        assert!(v[0].path.ends_with("templates/b/y.md"), "{:?}", v[0].path);
        assert!(v[0].message.contains("trusts__nevada"));
    }

    #[test]
    fn engine_scans_readme_claude_but_skips_hidden_dirs_and_target() {
        let dir = TempDir::new().unwrap();
        // README/CLAUDE are no longer name-excluded — they are scanned like
        // any other markdown. Hidden directories and `target/` are still
        // skipped at the walk level.
        write(dir.path(), "README.md", &"x".repeat(200));
        write(dir.path(), "CLAUDE.md", &"x".repeat(200));
        write(dir.path(), ".hidden/inside.md", &"x".repeat(200));
        write(dir.path(), "target/build.md", &"x".repeat(200));
        write(
            dir.path(),
            "good.md",
            "---\ntitle: Good\nrespondent_type: entity\n---\n",
        );
        let report = RuleEngine::new(minimal_engine_rules())
            .lint_directory(dir.path())
            .unwrap();
        assert_eq!(report.files_scanned, 3);
        // good.md is clean; README/CLAUDE trip S101 (long line) and, lacking
        // frontmatter, the N101/N102 rules in this minimal set.
        assert!(report
            .violations
            .iter()
            .all(|v| v.path.ends_with("README.md") || v.path.ends_with("CLAUDE.md")));
        assert!(!report.is_clean());
    }
}
