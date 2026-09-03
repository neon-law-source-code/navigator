//! The Dioxus Components — the design system rebuilt as Dioxus components
//! (issue #641, Phase 2).
//!
//! These are the successors to the builders in `views::components`,
//! styled by the Dioxus Components theme (`server/public/css/theme.css`) rather
//! than Bootstrap. `/design` ([`crate::design`]) renders them as the living
//! contract. The set grows a batch per PR — this module is the migration
//! checklist.
//!
//! # The leaf rule
//!
//! This module is a **leaf**: it imports no application module. No router, no
//! session, no `AppState`, no data access — a component takes the data and the
//! callbacks it needs as props, and a navigable one takes an `href` and renders
//! a plain anchor. That is what lets the same component render a server-only
//! marketing page that ships no hydration bundle *and* an interactive
//! authenticated page: navigation is injected at the call site rather than
//! imported here.
//!
//! Colour is the same story one level down. A component emits a semantic class
//! name and every value resolves through a `--nav-*` custom property, so
//! whichever brands share this surface each supply their own values. A literal
//! colour in a component pins one brand's identity into code every brand consumes.
//!
//! Both rules are enforced by the tests at the bottom of this file rather than
//! stated only here — a boundary that lives in a comment erodes on the first
//! deadline.

pub mod app_footer;
pub mod app_navbar;
pub mod avatar;
pub mod breadcrumb;
pub mod card;
pub mod catalog_hero;
pub mod code;
pub mod confirm_delete;
pub mod copy_runs;
pub mod data_table;
pub mod disclaimer;
pub mod focus;
pub mod form;
pub mod github_stars;
pub mod icon;
pub mod impersonation;
pub mod links;
pub mod navigator_chrome;
pub mod pagination;
pub mod people_list;
pub mod person_picker;
pub mod platform_mark;
pub mod practice_card;
pub mod pricing;
pub mod public_shell;
pub mod resource_mark;
pub mod row_actions;
pub mod sample_matters_banner;
pub mod site_footer;
pub mod site_header;
pub mod social;
pub mod testimonial;
pub mod toast;

#[cfg(feature = "server")]
pub use app_footer::render_app_footer;
pub use app_footer::AppFooter;
pub use app_navbar::{AppLogo, AppNavLink, AppNavbar};
pub use avatar::{initials, Avatar};
pub use breadcrumb::{BackBreadcrumb, LawyerPortalBreadcrumb};
pub use card::Card;
pub use catalog_hero::{CatalogHero, CATALOG_STYLESHEET_HREF};
pub use code::CodeBlock;
pub use confirm_delete::ConfirmDelete;
pub use copy_runs::{wire_runs, CopyRun, RunParagraph};
pub use data_table::{Column, DataTable, Direction, SortState};
pub use disclaimer::LegalBlueprintDisclaimer;
pub use focus::{
    ChoiceGroup, ChoiceGroupOption, Hero, HeroAlign, HeroLevel, Stage, StageWidth, StepList,
    StepMeta, Stepper, StepperPanel,
};
#[cfg(test)]
pub(crate) use form::assert_forms_accessible;
pub use form::{
    question_fields, Choice, Field, FieldKind, FormCard, Heading, QuestionFieldContext,
};
pub use github_stars::GitHubStars;
pub use icon::{Icon, IconName, LIBRA_SCALES};
pub use impersonation::{
    Impersonating, ImpersonationBanner, ImpersonationView, IMPERSONATION_STOP_ACTION,
};
pub use links::ExternalLink;
pub use navigator_chrome::{
    NavigatorDestination, NavigatorFooter, NavigatorFooterLink, NavigatorNavbar, NavigatorShell,
};
pub use pagination::Pagination;
pub use people_list::PeopleListInputs;
pub use person_picker::{PersonChoice, PersonPicker};
pub use platform_mark::PlatformMark;
pub(crate) use platform_mark::PlatformMarkGlyph;
pub use practice_card::PracticeMark;
pub(crate) use practice_card::{PracticeCard, PracticeMarkGlyph};
pub use pricing::{PricingCard, PricingSection};
pub use public_shell::{PublicShell, PUBLIC_SHELL_MARKER};
pub use row_actions::RowActions;
#[cfg(feature = "server")]
pub use sample_matters_banner::render_sample_matters_banner;
pub use sample_matters_banner::{SampleMattersBanner, SAMPLE_MATTERS_BANNER_ID};
pub use site_footer::{
    FooterAttorney, FooterBarLicense, FooterNavLink, FooterOffice, SiteFooterLegal,
};
// `pub(crate)`, not `pub`: these two are reused by `crate::litigation_page`
// for the same channel links outside the footer, but they are not part of
// this crate's public component API.
pub(crate) use site_footer::{mailto_href, tel_href};
pub use site_header::{SiteHeader, SiteNavLink};
pub use social::SocialMeta;
pub use testimonial::{TestimonialCard, TestimonialSection};
pub use toast::{Toast, ToastTone};

/// The same-origin href of the Dioxus Components theme stylesheet, served by
/// `web`'s static `/public` route. Rendered into the page head by the pages
/// that adopt the theme (via `dioxus::document::Stylesheet`), so the not-yet
/// migrated pages keep Bootstrap until their Phase 3 cluster moves.
pub const THEME_STYLESHEET_HREF: &str = "/public/css/theme.css";

#[cfg(test)]
mod leaf_contract {
    use std::path::{Path, PathBuf};

    /// Read `path` as code: line comments dropped, and everything from this
    /// test module onwards discarded. Prose may *name* a forbidden crate to
    /// explain why it is forbidden; the rule is about what the code reaches
    /// for, so both tests read this rather than the raw file. Line numbers are
    /// preserved (a stripped line becomes an empty one) so a failure points at
    /// the real line.
    fn code_of(path: &Path) -> String {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let raw = raw.split("mod leaf_contract").next().unwrap_or_default();
        raw.lines()
            .map(|line| {
                if line.trim_start().starts_with("//") {
                    ""
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every source file that makes up the theme: this module plus each
    /// component beside it, as code.
    fn component_sources() -> Vec<(PathBuf, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = vec![root.join("src/components.rs")];
        let dir = std::fs::read_dir(root.join("src/components")).expect("read src/components");
        for entry in dir {
            let path = entry.expect("dir entry").path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
        files.sort();
        files
            .into_iter()
            .map(|path| {
                let body = code_of(&path);
                (path, body)
            })
            .collect()
    }

    /// The application crates and identifiers a themed component may never
    /// reach for. `portal` is the router and application state, `server` the
    /// binary that mounts it, `store`/`cloud` the data and object
    /// layers, and `dioxus_router` the navigation import the injected-link
    /// contract exists to avoid.
    ///
    /// `views` is deliberately absent: `code.rs` calls its pure `syntect`
    /// highlighter inside a `#[server]` body. That is a rendering helper with
    /// no request, session, or route in it — extend this list, never that
    /// exception, when the boundary needs to tighten.
    const FORBIDDEN: &[&str] = &[
        "portal::",
        "use portal",
        "server::",
        "store::",
        "use store",
        "cloud::",
        "dioxus_router",
        "dioxus::router",
        "AppState",
        "SessionData",
        "use_navigator",
        "use_route",
    ];

    /// The theme imports from no application module. The TypeScript design
    /// system this theme replaced enforced the same boundary with an executable
    /// test, and that is why it held; this is the Rust port of that test.
    #[test]
    fn components_import_no_app_crate() {
        for (path, source) in component_sources() {
            for needle in FORBIDDEN {
                assert!(
                    !source.contains(needle),
                    "{} reaches for `{needle}`. The Dioxus Components theme is a leaf: \
                     components take data and callbacks as props, and a navigable component \
                     takes an `href` and renders a plain anchor. Move the application \
                     dependency to the page that mounts the component.",
                    path.display(),
                );
            }
        }
    }

    /// Every public `#[component]` in the theme, by name.
    ///
    /// A private helper component (`fn Foo`, no `pub`) is an implementation
    /// detail of the component that owns it and renders only through that
    /// owner, so it is deliberately excluded: it reaches the gallery — and the
    /// accessibility gate — through its parent.
    fn public_theme_components() -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        for (path, source) in component_sources() {
            let mut lines = source.lines().peekable();
            while let Some(line) = lines.next() {
                if line.trim() != "#[component]" {
                    continue;
                }
                // The signature may not be the very next line (props
                // attributes, `#[allow]`), so scan forward to the first `fn`.
                for candidate in lines.by_ref() {
                    let trimmed = candidate.trim_start();
                    if let Some(rest) = trimmed.strip_prefix("pub fn ") {
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            found.push((path.clone(), name));
                        }
                        break;
                    }
                    if trimmed.starts_with("fn ") {
                        break;
                    }
                }
            }
        }
        found
    }

    /// Every public component appears in the `/design` gallery.
    ///
    /// This is the rule that keeps the accessibility gate from having to walk
    /// every page. `server/tests/accessibility_e2e.rs` audits `/design` as one
    /// full document, so a `FormCard` label defect or an unnamed icon button
    /// fails once, in the component gate, instead of being discovered
    /// separately on each of the twenty-odd pages that mount it. That only
    /// holds while the gallery is complete — a component absent from `/design`
    /// is a component no browser gate renders. So the gallery is not a
    /// hand-kept list that drifts: adding a public component to the theme
    /// fails this test until the component is shown, which is what makes the
    /// coverage maintain itself rather than needing a sweep of every page.
    #[test]
    fn every_public_component_is_shown_in_the_design_gallery() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let gallery = code_of(&root.join("src/design.rs"));

        let missing: Vec<String> = public_theme_components()
            .into_iter()
            .filter(|(_, name)| {
                // Word-boundary match: `Card` must not be satisfied by
                // `PricingCard`, or a renamed component would slip through on
                // a substring of an unrelated one.
                !gallery
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|token| token == name)
            })
            .map(|(path, name)| {
                format!(
                    "{name} ({})",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )
            })
            .collect();

        assert!(
            missing.is_empty(),
            "these public components are not rendered in `/design`:\n  {}\n\
             The gallery is what the accessibility gate audits, so a component \
             missing here is a component no browser test ever renders. Show it \
             in `webapp/src/design.rs` (or make it private, if it only ever \
             renders through another component).",
            missing.join("\n  "),
        );
    }

    /// A component declares no literal colour. Every value resolves through a
    /// `--nav-*` custom property, so one component surface serves every brand.
    #[test]
    fn components_declare_no_literal_colors() {
        // A hex colour (`#06b6d4`), or a functional colour notation. Matching
        // on the source text keeps the rule cheap and readable; the components
        // carry no CSS beyond the occasional custom-property passthrough.
        let literal_color = |line: &str| {
            ["rgb(", "rgba(", "hsl("].iter().any(|f| line.contains(f))
                || line.split('#').skip(1).any(|rest| {
                    let hex: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
                    matches!(hex.len(), 3 | 6)
                        && rest
                            .chars()
                            .nth(hex.len())
                            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
                })
        };

        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let gallery = root.join("src/design.rs");
        let mut sources = component_sources();
        sources.push((gallery.clone(), code_of(&gallery)));

        for (path, source) in sources {
            // `resource_mark` draws third-party logos — Slack's pinwheel,
            // Google Drive's triangle — in their owners' colours. Those are
            // exempt because the rule's reason does not reach them: a `--nav-*`
            // token exists so one component serves three *firm* brands, and
            // Slack's crimson is not one of our brands to re-theme. A
            // deployment-varying Slack logo would be a wrong Slack logo. Every
            // other component, this file included, still resolves every colour
            // through a token.
            if path
                .file_name()
                .is_some_and(|name| name == "resource_mark.rs")
            {
                continue;
            }
            for (number, line) in source.lines().enumerate() {
                assert!(
                    !literal_color(line),
                    "{}:{} declares a literal colour:\n{line}\nUse a `--nav-*` token: a literal \
                     value pins one brand's identity into shared code.",
                    path.display(),
                    number + 1,
                );
            }
        }
    }
}
