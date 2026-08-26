//! `/design` — the firm's living design system, rendered through Dioxus.
//!
//! The gallery renders the **Dioxus Components** this crate owns
//! (`crate::components`), styled by the Dioxus Components theme
//! (`server/public/css/theme.css`), so a contributor sees the real components
//! the pages use. Every block below is the production component, so the gallery
//! can never drift from what ships.
//!
//! Three contracts the theme inherits are visible here, and two of them are
//! enforced mechanically by the tests in [`crate::components`]:
//!
//! 1. **The leaf rule.** A themed component imports no application module — no
//!    router, no session, no `AppState`, no data access. It takes data and
//!    callbacks as props. `components_import_no_app_crate` walks the component
//!    sources and fails the build on a forbidden import.
//! 2. **Injected links.** A navigable component takes an `href` and renders a
//!    plain `<a>`. Nothing imports a router, so a themed component stays usable
//!    on an SSR-only page that ships no hydration bundle; a client that wants
//!    client-side navigation wraps the anchor at the call site.
//! 3. **Brand tokens.** Components emit semantic class names and every colour
//!    resolves through a `--nav-*` custom property, so every brand shares one
//!    surface. The palette section below draws each swatch from its token
//!    rather than a literal hex, and `components_declare_no_literal_colors`
//!    fails the build on a raw colour value in a component or in this gallery.
//!
//! The theme stylesheet is loaded through [`document::Stylesheet`] so it hoists
//! into the page head.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    wire_runs, AppFooter, AppLogo, AppNavbar, BackBreadcrumb, Card, CatalogHero, Choice, CodeBlock,
    Column, ConfirmDelete, DataTable, ExternalLink, Field, FooterAttorney, FooterBarLicense,
    FooterNavLink, FooterOffice, FormCard, GitHubStars, Icon, IconName, ImpersonationBanner,
    ImpersonationView, LawyerPortalBreadcrumb, LegalBlueprintDisclaimer, NavigatorDestination,
    NavigatorFooter, NavigatorFooterLink, NavigatorNavbar, NavigatorShell, Pagination,
    PeopleListInputs, PersonChoice, PersonPicker, PricingCard, PricingSection, PublicShell,
    RowActions, RunParagraph, SampleMattersBanner, SiteFooterLegal, SiteHeader, SiteNavLink,
    SocialMeta, SortState, TestimonialCard, TestimonialSection, Toast, ToastTone,
    THEME_STYLESHEET_HREF,
};
// The vendor marks come from their own module rather than the theme root: they
// are the one component whose colours are a third party's rather than the
// deployment's, so they are kept visibly apart from the themed set.
use crate::components::resource_mark::{ResourceMark, ResourceMarkGlyph};

/// The demo table's advertised sort keys — the JSON:API contract's allow-list.
/// The `/design` route pre-handler (`portal::dioxus_app::design_router`) `400`s a
/// `?sort=` naming anything else, and this module keeps the same list so the
/// gallery and the guard never disagree.
pub const DEMO_SORT_KEYS: &[&str] = &["name", "role"];

/// One row of the `/design` demo table. Firm-authored council personas — a
/// synthetic fixture, never client data.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DemoRow {
    pub name: String,
    pub role: String,
}

/// The demo table's URL contract: JSON:API `?sort=` plus a 1-indexed `?page=`.
#[derive(Deserialize, Default)]
pub struct DemoQuery {
    #[serde(default)]
    pub sort: Option<String>,
    /// 1-indexed page. Parsed leniently: a non-numeric `?page=` degrades to
    /// `None` (page 1 after the clamp below) instead of failing the whole query
    /// extraction and blanking the table.
    #[serde(default, deserialize_with = "deserialize_lenient_page")]
    pub page: Option<u32>,
    /// A non-authoritative name/email filter for the person-picker example.
    #[serde(default)]
    pub design_person_id_search: Option<String>,
}

/// Deserialize `?page=` leniently: a missing, blank, or non-numeric value
/// yields `None` (which [`load_demo_table`] clamps to page 1) rather than
/// failing the `Query` extraction and rendering the table's error state.
fn deserialize_lenient_page<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(parse_page(raw.as_deref()))
}

/// Parse a raw `?page=` value into a 1-indexed page number, or `None` when it
/// is absent, blank, or not a positive integer.
fn parse_page(raw: Option<&str>) -> Option<u32> {
    raw.and_then(|value| value.trim().parse::<u32>().ok())
}

/// The rendered demo-table view: the current page's rows, the active sort (so
/// the header anchors can build the toggle links), and the paging position.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DemoView {
    pub rows: Vec<DemoRow>,
    pub sort: String,
    pub page: u32,
    pub total_pages: u32,
}

/// The synthetic dataset — firm-authored council personas (name, role). Small
/// enough to page in fours so the pagination control has something to do.
#[cfg(feature = "server")]
const DEMO_PERSONAS: &[(&str, &str)] = &[
    ("Aquarius", "Platform engineer"),
    ("Aries", "Incident commander"),
    ("Cancer", "New hire"),
    ("Capricorn", "Graybeard"),
    ("Libra", "Prospective client"),
    ("Sagittarius", "Product manager"),
    ("Scorpio", "Security engineer"),
    ("Virgo", "Engineering manager"),
];

/// Rows per page on the demo table.
#[cfg(feature = "server")]
const DEMO_PAGE_SIZE: usize = 4;

/// Load the demo table for the current request: read `?sort=` / `?page=`, sort
/// the synthetic rows, and return the requested page. The body runs on the
/// server (the `#[server]` macro stubs it for the wasm client); `?sort=` has
/// already been validated to the advertised keys by the route pre-handler, so
/// an unknown key never reaches here.
#[server]
pub async fn load_demo_table() -> Result<DemoView, ServerFnError> {
    let axum::extract::Query(query) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<DemoQuery>, _>()
            .await?;
    let sort = query.sort.unwrap_or_default();

    let mut rows: Vec<DemoRow> = DEMO_PERSONAS
        .iter()
        .map(|(name, role)| DemoRow {
            name: (*name).to_string(),
            role: (*role).to_string(),
        })
        .collect();

    // Sort by every advertised `?sort=` field in order, so a later field is the
    // tiebreaker for the earlier ones — the JSON:API multi-field contract this
    // reference implements. Unknown keys never reach here (the pre-handler
    // `400`s them) and contribute no ordering.
    sort_demo_rows(&mut rows, &sort);

    let total_pages = u32::try_from(rows.len().div_ceil(DEMO_PAGE_SIZE).max(1)).unwrap_or(u32::MAX);
    let page = query.page.unwrap_or(1).clamp(1, total_pages);
    let start = (page as usize - 1) * DEMO_PAGE_SIZE;
    let rows = rows.into_iter().skip(start).take(DEMO_PAGE_SIZE).collect();

    Ok(DemoView {
        rows,
        sort,
        page,
        total_pages,
    })
}

/// Read the person-picker's non-authoritative query filter from `/design`.
#[server]
pub async fn load_demo_person_search() -> Result<Option<String>, ServerFnError> {
    let axum::extract::Query(query) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<DemoQuery>, _>()
            .await?;
    Ok(query.design_person_id_search)
}

/// Sort `rows` in place by a JSON:API `?sort=` value, applying each advertised
/// field in order (a later field is the tiebreaker for the earlier ones). An
/// empty spec leaves the source order; an unadvertised key (impossible past the
/// route guard) contributes no ordering. Server-only — the sole caller is the
/// server function.
#[cfg(feature = "server")]
fn sort_demo_rows(rows: &mut [DemoRow], sort: &str) {
    let fields: Vec<(&str, bool)> = sort
        .split(',')
        .map(str::trim)
        .filter_map(|segment| match segment.strip_prefix('-') {
            Some(key) if !key.is_empty() => Some((key, true)),
            None if !segment.is_empty() => Some((segment, false)),
            // A lone `-` (empty key) or an empty segment contributes no field.
            _ => None,
        })
        .collect();

    rows.sort_by(|a, b| {
        for (key, descending) in &fields {
            let ordering = match *key {
                "name" => a.name.cmp(&b.name),
                "role" => a.role.cmp(&b.role),
                _ => std::cmp::Ordering::Equal,
            };
            let ordering = if *descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// One grounded code sample on the gallery, copied verbatim from a real
/// component source. `code` is an exact substring of the crate-relative
/// `source` file; the `snippets_are_exact_copies_of_cited_sources` test reads
/// each `source` and fails the build if a snippet drifts.
struct CodeSnippet {
    /// Crate-relative path (under `webapp/`) to the file this is copied from.
    source: &'static str,
    /// What the snippet demonstrates.
    caption: &'static str,
    /// The code, verbatim from `source`.
    code: &'static str,
}

/// The gallery's grounded snippets — real component source a contributor can
/// copy, each guarded by the drift test below.
const SNIPPETS: &[CodeSnippet] = &[
    CodeSnippet {
        source: "src/components/card.rs",
        caption: "The Card component",
        code: "#[component]
pub fn Card(
    children: Element,",
    },
    CodeSnippet {
        source: "src/components/icon.rs",
        caption: "The inline-SVG Icon component",
        code: "#[component]
pub fn Icon(name: IconName, #[props(default)] label: Option<String>) -> Element {",
    },
    CodeSnippet {
        source: "src/components/links.rs",
        caption: "The injected-link contract — an href prop, a plain anchor",
        code: "#[component]
pub fn ExternalLink(
    href: String,",
    },
];

/// The demo data table. Server-side rendered from `?sort=` / `?page=` via
/// [`use_server_future`], so the sorted, paged rows are in the pre-hydration
/// HTML and the header + pagination anchors work without JS. This is the
/// reference every list page copies.
#[component]
fn DemoTableSection() -> Element {
    let resource = use_server_future(load_demo_table)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => return rsx! { p { "Failed to load the demo table." } },
        None => return rsx! { p { "Loading…" } },
    };

    let sort = SortState::parse(Some(&view.sort));
    let columns = vec![
        Column::sortable("name", "Name"),
        Column::sortable("role", "Role"),
    ];
    // Paging preserves the active sort; a sort click resets to page 1 (so it is
    // deliberately not carried onto the header links).
    let page_query = if view.sort.is_empty() {
        Vec::new()
    } else {
        vec![("sort".to_string(), view.sort.clone())]
    };

    rsx! {
        DataTable {
            columns,
            sort,
            base_path: "/design".to_string(),
            for row in view.rows.iter() {
                tr { key: "{row.name}",
                    td { "{row.name}" }
                    td { "{row.role}" }
                }
            }
        }
        Pagination {
            current: view.page,
            total: view.total_pages,
            base_path: "/design".to_string(),
            extra_query: page_query,
        }
    }
}

/// The brand tokens the gallery previews, each drawn from its own `--nav-*`
/// custom property. The swatch chip resolves the token at paint time, so this
/// page shows the *running deploy's* brand rather than a hard-coded ramp — the
/// property that lets every brand share one component surface.
const SWATCH_TOKENS: &[&str] = &[
    "--nav-color-primary",
    "--nav-color-primary-hover",
    "--nav-color-primary-active",
    "--nav-color-link",
    "--nav-color-surface",
    "--nav-color-surface-raised",
    "--nav-color-surface-subtle",
    "--nav-color-border",
    "--nav-color-success",
    "--nav-color-danger",
    "--nav-color-warning",
];

/// The icons on the gallery, each with the label it reads as when meaningful.
const ICONS: &[(IconName, &str)] = &[
    (IconName::LibraScales, "Litigation"),
    (IconName::StarFill, "Featured"),
    (IconName::ShieldFillCheck, "Verified"),
    (IconName::PencilSquare, "Edit"),
    (IconName::Trash3Fill, "Delete"),
    (IconName::Eye, "View"),
    (IconName::Github, "GitHub"),
    (IconName::Diagram3Fill, "Structure"),
    (IconName::CheckLg, "Done"),
    (IconName::XLg, "Dismiss"),
];

/// The `/design` gallery component.
#[allow(non_snake_case)]
pub fn DesignGallery() -> Element {
    rsx! {
        document::Title { "Design system — Neon Law Navigator" }
        document::Stylesheet { href: THEME_STYLESHEET_HREF }
        // The social-share preview tags hoist into <head>, resolved from the
        // deploy's branding so a white-label install emits its own identity.
        SocialMetaSection {}
        main { id: "design", class: "nav-theme design-gallery",
            h1 { "Design system" }
            p {
                "The firm's living design system, rendered through Dioxus. Every \
                 block below is the real component the pages use, styled by the \
                 Dioxus Components theme — no Bootstrap. Dark mode follows your \
                 OS setting through "
                code { "prefers-color-scheme" }
                ", with no pre-paint script."
            }

            ContractsSection {}
            PaletteSection {}
            IconsSection {}

            section {
                h2 { "Cards" }
                div { class: "design-cards",
                    Card {
                        header: rsx! { "Plain card" },
                        p { "The shared surface — a header band, a body, and an optional footer." }
                    }
                    Card {
                        highlighted: true,
                        header: rsx! { "Recommended" },
                        p { "The brand anchor treatment for a \"this one\" card." }
                    }
                    Card {
                        footer: rsx! {
                            a { href: "/design", "Footer action" }
                        },
                        p { "A card with a footer band for secondary actions." }
                    }
                }
            }

            section {
                h2 { "Toasts" }
                div { class: "design-toasts",
                    Toast { message: "Sign in to continue.".to_string(), tone: ToastTone::Danger }
                    Toast { message: "Your draft was saved.".to_string(), tone: ToastTone::Primary }
                    Toast { message: "Matter opened.".to_string(), tone: ToastTone::Success }
                    Toast { message: "Heads up — review pending.".to_string(), tone: ToastTone::Warning }
                }
            }

            section {
                h2 { "Data table" }
                p {
                    "The URL-contract reference: sort state lives in "
                    code { "?sort=" }
                    " and paging in "
                    code { "?page=" }
                    ", both as real anchors, so the table works before hydration and for \
                     crawlers. A click on a sortable header navigates; the server re-sorts \
                     and re-renders. A "
                    code { "?sort=" }
                    " naming an unadvertised field returns "
                    code { "400" }
                    "."
                }
                DemoTableSection {}
            }

            RowActionsShowcase {}
            ConfirmDeleteShowcase {}
            PricingShowcase {}
            TestimonialShowcase {}
            DisclaimerShowcase {}
            ImpersonationShowcase {}
            SampleMattersShowcase {}
            CopyRunsShowcase {}
            ResourceMarkShowcase {}
            CatalogHeroShowcase {}
            SiteHeaderShowcase {}
            SiteFooterShowcase {}
            PublicShellShowcase {}
            FormShowcase {}
            PeopleListShowcase {}
            AppNavbarShowcase {}
            AppFooterShowcase {}
            NavigatorChromeShowcase {}
            NavigationShowcase {}
            SnippetsSection {}
        }
    }
}

/// The three contracts the Dioxus Components theme carries, stated on the page
/// a contributor actually reads before adding a component. Each is enforced by
/// a test, named here so the reader can find the gate rather than trust prose.
#[component]
fn ContractsSection() -> Element {
    rsx! {
        section {
            h2 { "The three contracts" }
            ul {
                li {
                    strong { "The leaf rule." }
                    " A themed component imports no application module — no router, no session, \
                     no application state, no data access. It takes data and callbacks as props. \
                     Enforced by "
                    code { "components_import_no_app_crate" }
                    ", which walks the component sources and fails the build on a forbidden \
                     import. A boundary that lives only in a document erodes on the first \
                     deadline."
                }
                li {
                    strong { "Injected links." }
                    " A navigable component takes an "
                    code { "href" }
                    " and renders a plain anchor. Nothing here imports a router, which is what \
                     lets these components render a server-only page that ships no hydration \
                     bundle; a client that wants client-side navigation supplies it at the call \
                     site."
                }
                li {
                    strong { "Brand tokens." }
                    " Components emit semantic class names and every colour resolves through a "
                    code { "--nav-*" }
                    " custom property, so every brand shares one surface. A literal colour in a \
                     component pins one brand's identity into code every brand consumes; "
                    code { "components_declare_no_literal_colors" }
                    " fails the build on one."
                }
            }
        }
    }
}

/// The `/app` navbar, at each tier. The gallery supplies the destinations
/// literally and a synthetic mark; a real page resolves both from the request
/// (`crate::app_chrome`) — the viewer's role decides which workspaces appear and
/// the mounted brand decides the mark.
#[component]
fn AppNavbarShowcase() -> Element {
    let tiers = [
        ("Client", crate::people::ViewerRole::Client),
        ("Lawyer", crate::people::ViewerRole::Lawyer),
        ("Admin", crate::people::ViewerRole::Admin),
    ];
    rsx! {
        section {
            h2 { "Application navbar" }
            p {
                "The one navbar every authenticated /app page renders. The firm workspaces are "
                "gated by the viewer's tier, and the deploy's brand mark sits at the trailing "
                "edge — a prop, so a white-label install publishes its own."
            }
            for (label, role) in tiers {
                h3 { "{label}" }
                AppNavbar {
                    destinations: crate::app_chrome::app_destinations(role),
                    logo: Some(AppLogo {
                        src: "/public/img/logo.svg".to_string(),
                        href: "/".to_string(),
                        brand_name: "Example Law".to_string(),
                    }),
                }
            }
        }
    }
}

/// The minimal footer every `/app` page carries, injected once into every
/// response by `portal::dioxus_app::dioxus_document_head` rather than
/// rendered by each of the eight real `/app` pages — see the component's own
/// module docs for why. It carries nothing but the copyright line.
#[component]
fn AppFooterShowcase() -> Element {
    rsx! {
        section {
            h2 { "Application footer" }
            p {
                "The one footer every authenticated /app page carries: a centered copyright "
                "line naming the entity of record, and nothing else."
            }
            AppFooter { legal_entity: "Shook Law PLLC".to_string(), copyright_year: 2026 }
        }
    }
}

/// The authenticated app's shared global chrome. The gallery supplies a
/// synthetic host-aware footer and a selected lawyer destination; real route
/// adapters provide the same model from the authenticated request.
#[component]
fn NavigatorChromeShowcase() -> Element {
    rsx! {
        section {
            h2 { "Authenticated Navigator chrome" }
            p {
                "Every authenticated surface shares one global navbar and footer. "
                "Routes supply only the destinations the current viewer may see, the active "
                "destination, and host-specific legal/release footer content."
            }
            div { class: "navigator-chrome-showcase",
                // The gallery already owns the page's `<main>` landmark, so the
                // preview renders the shell's content region as a plain element
                // rather than nesting a second `<main>` inside it.
                NavigatorShell {
                    main_landmark: false,
                    navbar: rsx! {
                        NavigatorNavbar {
                            brand_name: "Neon Law Navigator".to_string(),
                            brand_href: "/app/projects".to_string(),
                            destinations: vec![
                                NavigatorDestination::new("Portal", "/app/projects", false),
                                NavigatorDestination::new("Lawyer", "/app/lawyer", true),
                                NavigatorDestination::new("Admin", "/admin", false),
                            ],
                        }
                    },
                    footer: rsx! {
                        NavigatorFooter {
                            legal_attribution: "Legal services rendered by Example Firm PLLC."
                                .to_string(),
                            release_label: "Navigator 26.7.26".to_string(),
                            links: vec![
                                NavigatorFooterLink::new("Privacy", "/privacy"),
                                NavigatorFooterLink::new("Terms", "/terms"),
                            ],
                        }
                    },
                    div { class: "navigator-chrome-showcase__content",
                        h3 { "Lawyer workspace" }
                        p { "The page body stays route-specific inside the shared frame." }
                    }
                }
            }
        }
    }
}

/// The deploy's social-share identity: brand name plus the absolute logo URL
/// for `og:image`. Plain, wasm-safe fields so it crosses the server→client
/// boundary; the client build calls the generated stub.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SocialBranding {
    pub site_name: String,
    pub image: String,
}

/// Resolve the running deploy's social-share branding from the process brand
/// (`views::brand`), so a white-label install emits its own site name and logo
/// in the share preview instead of hard-coded Neon Law tags. `og:image` must be
/// absolute (scrapers drop relative URLs), so the firm's raster mark is resolved
/// against the site origin server-side.
#[server]
// A server function must be `async` (the macro requires it); this one resolves
// process branding synchronously, with nothing to await.
#[allow(clippy::unused_async)]
pub async fn design_social_branding() -> Result<SocialBranding, ServerFnError> {
    Ok(SocialBranding {
        site_name: views::brand::FIRM_BRAND.site_name.to_string(),
        image: views::assets::absolute_url(views::brand::FIRM_BRAND.social_image),
    })
}

/// The social-share preview tags, resolved from the deploy's branding and
/// hoisted into `<head>` during SSR. Its own section so the branding server
/// function suspends only this subtree; if branding cannot resolve, the rest of
/// the gallery still renders (the tags are a progressive enhancement).
#[component]
fn SocialMetaSection() -> Element {
    let resource = use_server_future(design_social_branding)?;
    let branding = match &*resource.read() {
        Some(Ok(branding)) => branding.clone(),
        // The tags are a progressive enhancement; if branding can't resolve,
        // render the gallery without them rather than failing.
        _ => return rsx! {},
    };
    rsx! {
        SocialMeta {
            title: format!("Design system — {}", branding.site_name),
            description: format!("{}'s living Dioxus design system.", branding.site_name),
            site_name: branding.site_name.clone(),
            image: branding.image,
        }
    }
}

/// The row-actions cell — the compact view/edit/delete control an admin table
/// row carries. Inline-SVG glyphs, theme buttons, a native `POST` delete (no
/// HTMX), and ARIA labels disambiguated by the row's identity.
#[component]
fn RowActionsShowcase() -> Element {
    rsx! {
        section {
            h2 { "Row actions" }
            p {
                "The per-row control cluster for an admin list. The glyphs are decorative inline \
                 SVG; each button's "
                code { "aria-label" }
                " carries the row's identity (\"Delete libra@example.com\"), and the delete is a \
                 native "
                code { "POST" }
                " form — no HTMX, no inline confirm handler (the strict "
                code { "script-src" }
                " forbids it)."
            }
            div { class: "nav-table-wrap",
                table { class: "nav-table",
                    thead { tr { th { scope: "col", "Person" } th { scope: "col", class: "nav-table__end", "" } } }
                    tbody {
                        tr {
                            td { "Libra Client " span { class: "nav-muted", "<libra@example.com>" } }
                            td { class: "nav-table__end",
                                // The demo controls point back at the GET-only `/design`
                                // gallery rather than at real lawyer routes: the view/edit
                                // links navigate to the gallery and the delete form submits
                                // with GET instead of POSTing a fake CSRF token into a
                                // protected (or unregistered) lawyer endpoint. The component
                                // still defaults `delete_method` to `post` for production.
                                RowActions {
                                    view_href: "/design".to_string(),
                                    edit_href: "/design".to_string(),
                                    delete_action: "/design".to_string(),
                                    delete_method: "get".to_string(),
                                    row_label: "libra@example.com".to_string(),
                                }
                            }
                        }
                        tr {
                            td { "Read-only row" }
                            td { class: "nav-table__end",
                                RowActions { view_href: "/design".to_string(), row_label: "the Atlas matter".to_string() }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The destructive-action confirmation card. An inline `confirm()` is
/// CSP-forbidden on the Dioxus pages, so the confirmation is a server-rendered
/// surface — a native-`POST` Confirm plus a Cancel link, no JS.
#[component]
fn ConfirmDeleteShowcase() -> Element {
    rsx! {
        section {
            h2 { "Confirm delete" }
            p {
                "A dedicated confirmation surface for destructive actions. The Confirm button is a \
                 native "
                code { "POST" }
                " to the real delete endpoint; Cancel is a plain link back. No inline "
                code { "confirm()" }
                " (the strict "
                code { "script-src" }
                " forbids it), so a stray click can't fire the delete."
            }
            // `/design` is a GET-only route, so the demo Confirm submits with
            // GET to navigate back to the gallery rather than POST into a 405.
            // The component still defaults `confirm_method` to `post` for the
            // production delete surfaces.
            ConfirmDelete {
                title: "Delete person".to_string(),
                message: "Delete libra@example.com? This cannot be undone.".to_string(),
                confirm_action: "/design".to_string(),
                confirm_method: "get".to_string(),
                cancel_href: "/design".to_string(),
            }
        }
    }
}

/// The pricing / offer cards section.
#[component]
fn PricingShowcase() -> Element {
    let cards = vec![
        PricingCard {
            title: "LLC formation".to_string(),
            price: "$1,000".to_string(),
            cadence: None,
            blurb: "For getting a business off the ground.".to_string(),
            features: vec![
                "Attorney-drafted articles".to_string(),
                "Registered-agent setup".to_string(),
            ],
            cta_label: "Start a matter".to_string(),
            cta_href: "/contact".to_string(),
            featured_label: Some("$1,000, once".to_string()),
        },
        PricingCard {
            title: "Living trust".to_string(),
            price: "$3,500".to_string(),
            cadence: None,
            blurb: "For planning your legacy.".to_string(),
            features: vec![
                "Attorney-drafted trust".to_string(),
                "Funding guidance".to_string(),
            ],
            cta_label: "Book a call".to_string(),
            cta_href: "https://cal.example/book".to_string(),
            featured_label: Some("$3,500, once".to_string()),
        },
    ];
    rsx! {
        section {
            h2 { "Pricing cards" }
            PricingSection { cards, cols_lg: 2 }
        }
    }
}

/// The testimonial cards section.
#[component]
fn TestimonialShowcase() -> Element {
    let cards = vec![TestimonialCard {
        quote: "They opened my matter in a day and kept me posted the whole way.".to_string(),
        attribution: "Aries Ram".to_string(),
        detail: Some("LLC formation".to_string()),
        profile_image_url: None,
        product_label: Some("Namesake".to_string()),
    }];
    rsx! {
        section {
            TestimonialSection {
                heading: "What clients say".to_string(),
                lead: "Real outcomes, in the clients' words.".to_string(),
                cards,
            }
        }
    }
}

/// The legal disclaimer partial.
#[component]
fn DisclaimerShowcase() -> Element {
    rsx! {
        section {
            h2 { "Legal disclaimer" }
            LegalBlueprintDisclaimer {}
        }
    }
}

/// The public page shell — the skeleton every public page wraps its content in,
/// composing the site header, a `main` content region, and the footer legal
/// strip. Rendered in preview mode (`main_landmark: false`) so it does not nest
/// a second `<main>` inside the gallery's own landmark.
#[component]
fn PublicShellShowcase() -> Element {
    rsx! {
        section {
            h2 { "Public page shell" }
            p {
                "The skeleton every public page wraps its content in: the "
                "site header, a "
                code { "main" }
                " region, and the footer legal strip, resolved from the process brand."
            }
            div { class: "public-shell-showcase",
                PublicShell {
                    main_landmark: false,
                    header: rsx! {
                        SiteHeader {
                            brand_name: "Neon Law".to_string(),
                            home_href: "/".to_string(),
                            logo_href: "/public/img/logo.svg".to_string(),
                            destinations: vec![
                                SiteNavLink::new("Services", "/#services"),
                                SiteNavLink::new("Blog", "/blog"),
                                SiteNavLink::new("Team", "/team").current(),
                                SiteNavLink::new("Contact", "/contact"),
                            ],
                            // The gallery is the one page carrying two headers,
                            // so this one holds its own disclosure state rather
                            // than repeating the default id.
                            menu_id: "design-shell-menu".to_string(),
                        }
                    },
                    footer: rsx! {
                        SiteFooterLegal {
                            copyright_holder: "Neon Law".to_string(),
                            disclaimer: "This site is attorney advertising. Prior results do not \
                                         guarantee a similar outcome."
                                .to_string(),
                            copyright_year: 2026,
                        }
                    },
                    h3 { "Page content" }
                    p { "Each page renders its own body here inside the shared frame." }
                }
            }
        }
    }
}

/// The impersonation banner an admin sees on every page while acting as a
/// client. Shown with a live view because the component renders nothing at all
/// for `None`, and a component that renders nothing is a component the
/// accessibility gate cannot audit.
///
/// The stop control is a real `POST` form. The demo carries no CSRF token, so
/// the hidden field is absent exactly as it is on a middleware-free path.
#[component]
fn ImpersonationShowcase() -> Element {
    rsx! {
        section {
            h2 { "Impersonation banner" }
            p {
                "When an admin acts as a client, every page says so and offers the way \
                 out. It is a "
                code { "role=\"status\"" }
                " region — ambient state, announced without interrupting."
            }
            ImpersonationBanner {
                view: ImpersonationView {
                    target_name: "Virgo Ramirez".to_string(),
                    target_email: "virgo@example.com".to_string(),
                    csrf_token: String::new(),
                },
            }
        }
    }
}

/// The site-wide notice a deployment holding invented matters publishes.
///
/// Shown here so the accessibility gate audits its contrast: the ground and
/// ink are the one token pair in `tokens.css` that does *not* flip between
/// themes, because a reader must not be able to miss this because their OS is
/// in dark mode.
#[component]
fn SampleMattersShowcase() -> Element {
    rsx! {
        section {
            h2 { "Sample-matter banner" }
            p {
                "A deployment whose matters are invented says so on every page. It is \
                 injected into every HTML response rather than rendered per page, because \
                 the pages that carried it would teach a reader to trust its absence on \
                 the ones that did not — the error pages above all."
            }
            SampleMattersBanner {}
        }
    }
}

/// Run-marked prose: the shape the firm's practice copy is authored in, where
/// emphasised phrases are data rather than markup baked into a string.
#[component]
fn CopyRunsShowcase() -> Element {
    rsx! {
        section {
            h2 { "Run-marked prose" }
            p {
                "Marketing copy arrives as runs — plain text with the emphasised \
                 phrases marked — so the emphasis survives a copy edit and never \
                 becomes HTML inside a content string."
            }
            RunParagraph {
                class: "design-copy-runs".to_string(),
                runs: wire_runs(vec![
                    ("The firm takes ".to_string(), false),
                    ("complex commercial litigation".to_string(), true),
                    (
                        " and flat-fee transactional work, priced through ".to_string(),
                        false,
                    ),
                    ("one conversation".to_string(), true),
                    (".".to_string(), false),
                ]),
            }
        }
    }
}

/// The vendor marks that open a matter's collaboration-resource rows.
///
/// Shown with the label each one actually ships beside, because the label is
/// what carries the row's meaning: the mark says *which service* and the label
/// says *which audience*. A reader auditing this section should see that no
/// mark is asked to convey "private" or "shared" on its own.
///
/// These are the one exception to the theme's no-literal-colour rule — a
/// vendor's own colours are not ours to re-theme per deployment brand. See
/// `webapp::components::resource_mark`.
#[component]
fn ResourceMarkShowcase() -> Element {
    rsx! {
        section {
            h2 { "Resource marks" }
            p {
                "The matter workbench links out to the places work happens. Each \
                 row opens on the service's own mark so six links are told apart \
                 by shape before a word is read, and the label names the audience \
                 the mark cannot show."
            }
            ul { class: "project-resources__list",
                for (mark, label) in [
                    (ResourceMark::Slack, "Private Slack channel"),
                    (ResourceMark::Notion, "Private Notion page"),
                    (ResourceMark::GoogleDrive, "Private Google Drive"),
                    (ResourceMark::Portal, "Client portal"),
                ] {
                    li { key: "{mark.name()}", class: "project-resources__row",
                        span { class: "project-resources__link",
                            ResourceMarkGlyph {
                                mark,
                                class: "project-resources__mark".to_string(),
                            }
                            span { class: "project-resources__label", "{label}" }
                        }
                    }
                }
            }
        }
    }
}

/// The catalog hero workshop and presentation pages open with.
///
/// This is the one showcase that renders a second `<h1>` on the gallery page:
/// the hero *is* a page's first-level heading, so showing it faithfully means
/// showing that. WCAG A/AA does not restrict a document to one `<h1>` (the
/// one-h1 rule axe carries is best-practice, outside the tags the gate runs),
/// and rendering it with a downgraded heading would audit a component that
/// does not ship.
#[component]
fn CatalogHeroShowcase() -> Element {
    rsx! {
        section {
            h2 { "Workshop and presentation hero" }
            CatalogHero {
                eyebrow: "For the hackers".to_string(),
                title: "Rust in Peace".to_string(),
                lede: "How the firm uses Rust to improve access to justice — \
                       deterministic workflows from law, one attorney-gated step at a \
                       time."
                    .to_string(),
            }
        }
    }
}

/// The public site header — the brand mark and the primary marketing nav, with
/// the active page marked. The gallery shows the signed-in variant so both the
/// primary destinations and the trailing utility group are visible.
#[component]
fn SiteHeaderShowcase() -> Element {
    rsx! {
        section {
            h2 { "Site header" }
            SiteHeader {
                brand_name: "Neon Law".to_string(),
                home_href: "/".to_string(),
                logo_href: "/public/img/logo.svg".to_string(),
                destinations: vec![
                    SiteNavLink::new("Services", "/#services"),
                    SiteNavLink::new("Blog", "/blog"),
                    SiteNavLink::new("Design", "/design").current(),
                    SiteNavLink::new("Team", "/team"),
                    SiteNavLink::new("Contact", "/contact"),
                ],
                utility: vec![SiteNavLink::new("Portal", "/app/projects")],
            }
        }
    }
}

/// The site footer's two bands — the contact band (email CTA, voice line,
/// offices) above the brand-driven legal strip (the copyright that names the
/// regulated entity, the per-attorney bar licences, and the
/// attorney-advertising disclaimer). A deploy that publishes no contact
/// channels renders the legal strip alone.
#[component]
fn SiteFooterShowcase() -> Element {
    rsx! {
        section {
            h2 { "Site footer" }
            SiteFooterLegal {
                copyright_holder: "Neon Law".to_string(),
                disclaimer: "This site is attorney advertising. Prior results do not \
                             guarantee a similar outcome."
                    .to_string(),
                copyright_year: 2026,
                brand_name: "Neon Law".to_string(),
                // The shipped footer's mark is a link home, so the gallery
                // shows it as one — an unlinked mark here would document a
                // footer no page renders.
                home_href: "/".to_string(),
                contact_email: "support@example.com".to_string(),
                phone: "+1 555 010 0100".to_string(),
                offices: demo_offices(),
                attorneys: demo_attorneys(),
                // The routes the header no longer carries, at the full length
                // the site publishes them: eight links, which the stylesheet
                // lays out as two columns of four. A gallery driving three
                // would document a row that never renders and would not show
                // the two-column layout at all.
                nav: [
                    ("Blog", "/blog"),
                    ("Contact", "/contact"),
                    ("Docs", "/docs"),
                    ("Navigator", "/navigator"),
                    ("Presentations", "/presentations"),
                    ("Privacy", "/privacy"),
                    ("Terms", "/terms"),
                    ("Workshops", "/workshops"),
                ]
                .into_iter()
                .map(|(label, href)| FooterNavLink {
                    label: label.to_string(),
                    href: href.to_string(),
                })
                .collect(),
                // The mark notice, driven with the firm's real registration
                // rather than a synthetic one. The offices and attorneys above
                // are fixtures because they stand in for regulated firm detail
                // a gallery has no business asserting; a registration is a
                // public record, and a made-up number beside a real-looking
                // link is the one thing this line must never show.
                trademark: "NEON LAW".to_string(),
                trademark_registration: "6,325,650".to_string(),
                trademark_record_url:
                    "https://tmsearch.uspto.gov/search/search-results/90039224".to_string(),
                // The open-source line, driven with a count so the gallery
                // shows the shape the deployed footer renders. The
                // count-less variant is shown beside the standalone
                // `GitHubStars` in "Navigation & links".
                source_repo: "neon-law-source-code/navigator".to_string(),
                source_href: "https://github.com/neon-law-source-code/navigator".to_string(),
                source_stars: 1234u64,
                // The release stamp, driven with a literal so the gallery shows
                // the line a deployed page renders. A local `cargo run` leaves
                // it unset and the site publishes no version.
                navigator_version: "26.8.20".to_string(),
                navigator_href: "/navigator".to_string(),
            }
        }
    }
}

/// Two sample offices for the footer showcase. Synthetic, like every other
/// gallery fixture: the real ones live in `views::brand`. One carries a note so
/// the gallery shows the qualified variant next to the bare one.
fn demo_offices() -> Vec<FooterOffice> {
    [
        ("California", "1 Broadway, Oakland, CA 94607", None),
        (
            "Nevada",
            "2 Virginia St, Reno, NV 89501",
            Some("Bar admission pending"),
        ),
    ]
    .into_iter()
    .map(|(state, address, note)| FooterOffice {
        state: state.to_string(),
        address: address.to_string(),
        note: note.map(str::to_string),
    })
    .collect()
}

/// One sample attorney holding two bar licences, so the showcase shows both the
/// per-attorney line and the multi-licence separator.
fn demo_attorneys() -> Vec<FooterAttorney> {
    vec![FooterAttorney {
        name: "Ada Lovelace".to_string(),
        licenses: [("California", "100001"), ("Nevada", "100002")]
            .into_iter()
            .map(|(jurisdiction, number)| FooterBarLicense {
                jurisdiction: jurisdiction.to_string(),
                number: number.to_string(),
                license_url: format!("https://example.com/bar/{number}"),
            })
            .collect(),
    }]
}

/// The create/edit form card. The plain-textarea field kind is the composer for
/// long-form input: the theme ships no rich-text editor.
///
/// The second card shows the rejected state — Rails' `field_with_errors`: the
/// reason sits with the control that caused it, the submitted value survives
/// so nothing is retyped, and the control is `aria-invalid` with the message in
/// its `aria-describedby`. No JavaScript: the handler redirects back with the
/// message and the values.
#[component]
fn FormShowcase() -> Element {
    let search = use_server_future(load_demo_person_search)?;
    let person_search = match &*search.read() {
        Some(Ok(value)) => value.clone(),
        Some(Err(_)) | None => None,
    };
    let fields = vec![
        Field::text("Full name", "name", "").required(),
        Field::email("Email", "email", "").help("We'll only use this to reply."),
        Field::select(
            "Practice area",
            "area",
            vec![
                Choice::new("formation", "Business formation"),
                Choice::new("estate", "Estate planning"),
                Choice::new("litigation", "Litigation"),
            ],
            Some("estate".to_string()),
        ),
        Field::textarea(
            "How can we help?",
            "message",
            "I'd like to plan my estate.",
            4,
        ),
    ];
    // A control's `id` defaults to its field name, which is right for a page
    // carrying one form and wrong for a page carrying two: this card repeats
    // the `name` and `email` fields above, so without explicit ids the page
    // ships duplicate `id`s and two labels pointing at the same control —
    // `duplicate-id-aria` and `form-field-multiple-labels`, both of which the
    // `/design` component audit reports. Naming them here is also the usage
    // any production page with two form cards has to follow.
    let rejected = vec![
        Field::text("Full name", "name", "Ada Lovelace")
            .id("rejected-name")
            .required(),
        Field::email("Email", "email", "ada@")
            .id("rejected-email")
            .required()
            .help("We'll only use this to reply.")
            .error("Enter a valid email address."),
    ];
    let people = vec![
        PersonChoice::new("demo-ada", "Ada Lovelace", "ada@example.com"),
        PersonChoice::new("demo-linus", "Linus Torvalds", "linus@example.com"),
        PersonChoice::new("demo-margaret", "Margaret Hamilton", "margaret@example.com"),
    ];
    rsx! {
        section {
            h2 { "Form" }
            FormCard {
                title: "Contact the firm".to_string(),
                action: "/design".to_string(),
                // `/design` is a GET-only route, so the demo submits with GET to
                // navigate back to the gallery rather than POST into a 405. The
                // component still defaults to `post` for the production forms.
                method: "get".to_string(),
                submit_label: "Send".to_string(),
                heading: crate::components::Heading::H2,
                fields,
            }
            h3 { "Rejected" }
            FormCard {
                title: "Contact the firm".to_string(),
                action: "/design".to_string(),
                method: "get".to_string(),
                submit_label: "Send".to_string(),
                heading: crate::components::Heading::H2,
                fields: rejected,
            }
            h3 { "Person foreign key" }
            p {
                "A native person-id field: the person’s name and email make the choice clear, \
                 while the submitted value remains the foreign-key id. Type a name or email to \
                 narrow the choices; select Filter people to update the list."
            }
            FormCard {
                title: "Person picker".to_string(),
                action: "/design".to_string(),
                method: "get".to_string(),
                submit_label: "Choose person".to_string(),
                heading: crate::components::Heading::H2,
                fields: Vec::new(),
                extra_fields: Some(rsx! {
                    PersonPicker {
                        label: "Client".to_string(),
                        name: "design_person_id".to_string(),
                        blank_label: "— pick a person —".to_string(),
                        people,
                        search: person_search,
                        help: Some("Search matches either a person’s name or email address.".to_string()),
                        required: true,
                    }
                }),
            }
        }
    }
}

/// The people-list question widget — bounded person-row groups.
#[component]
fn PeopleListShowcase() -> Element {
    let prior =
        r#"[{"name": "Aries Ram", "email": "aries@example.com", "title": "Managing member"}]"#;
    rsx! {
        section {
            h2 { "People list" }
            p { "A bounded set of person rows for questions like \"who are the managing members?\", pre-filled from a prior answer." }
            // `/design` is a GET-only route, so the demo submits with GET to
            // navigate back to the gallery rather than POST into a 405. The
            // production questionnaire posts these inputs with the rest of its
            // form.
            form { action: "/design", method: "get",
                PeopleListInputs { prior_json: prior.to_string(), rows: 2 }
            }
        }
    }
}

/// Navigation + link chrome: the breadcrumbs and an off-site link. Every one of
/// these takes an `href` and emits a plain anchor — the injected-link contract,
/// visible.
#[component]
fn NavigationShowcase() -> Element {
    rsx! {
        section {
            h2 { "Navigation & links" }
            p {
                "Each control below takes an "
                code { "href" }
                " and renders a plain anchor, so it works with no client bundle. A client that \
                 wants client-side navigation wraps the anchor where it mounts the component; \
                 nothing here imports a router."
            }
            BackBreadcrumb { href: "/".to_string(), label: "Back to home".to_string() }
            LawyerPortalBreadcrumb {}
            p {
                "An off-site link opens in a new tab with the OWASP "
                code { "rel" }
                " pair and an arrow: "
                ExternalLink { href: "https://www.neonlaw.com/navigator".to_string(), "Neon Law Navigator" }
                "."
            }
            p {
                "The footer's source-repository line. The count is a prop, read per request from \
                 a cache a background task refreshes — the component never fetches, which is why \
                 it renders here with no network at all. Both states are shown, because the \
                 second is the ordinary one before the first refresh lands:"
            }
            GitHubStars {
                href: "https://github.com/neon-law-source-code/navigator".to_string(),
                repo: "neon-law-source-code/navigator".to_string(),
                stars: 1234u64,
            }
            GitHubStars {
                href: "https://github.com/neon-law-source-code/navigator".to_string(),
                repo: "neon-law-source-code/navigator".to_string(),
            }
        }
    }
}

/// The brand-token swatches. Each chip resolves its own `--nav-*` property, so
/// the gallery shows the running deploy's brand rather than a hard-coded ramp.
#[component]
fn PaletteSection() -> Element {
    rsx! {
        section {
            h2 { "Brand tokens" }
            p {
                "Each chip paints "
                code { "var(--nav-…)" }
                " rather than a literal colour, so this page renders whichever brand the \
                 process is running as. A component that hard-codes a value pins one of three \
                 brands into shared code."
            }
            p {
                "Neon Law uses copper as its primary. Read the semantic token instead of \
                 carrying a brand's colour into a shared component."
            }
            div { class: "design-swatches",
                for token in SWATCH_TOKENS.iter() {
                    div { class: "design-swatch",
                        span {
                            class: "design-swatch__chip",
                            style: "--design-swatch-value: var({token});",
                        }
                        code { "{token}" }
                    }
                }
            }
        }
    }
}

/// The inline-SVG icon gallery section.
#[component]
fn IconsSection() -> Element {
    rsx! {
        section {
            h2 { "Icons" }
            p { "Inline SVG — no icon font. Each inherits the surrounding text size and color." }
            div { class: "design-icons",
                for (name , label) in ICONS.iter() {
                    span { class: "design-icon",
                        span { class: "design-icon__glyph",
                            Icon { name: *name, label: label.to_string() }
                        }
                        code { "{name.glyph()}" }
                    }
                }
            }
        }
    }
}

/// The grounded component-source snippets section.
#[component]
fn SnippetsSection() -> Element {
    rsx! {
        section {
            h2 { "Component source" }
            p { "Copy-paste-ready, and grounded: a drift test fails the build if any snippet stops matching its source file." }
            for snippet in SNIPPETS.iter() {
                figure {
                    figcaption { "{snippet.caption} — "
                        code { "webapp/{snippet.source}" }
                    }
                    // Highlighted server-side by syntect (the CodeBlock component),
                    // so the coloured tokens are in the pre-hydration HTML with no
                    // client highlighter.
                    CodeBlock { code: snippet.code.to_string() }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_page, SNIPPETS};

    /// A stray, blank, or absent `?page=` degrades to page 1 (`None`) instead of
    /// failing the query extraction and blanking the table; a positive integer
    /// reads through.
    #[test]
    fn parse_page_is_lenient_about_junk() {
        assert_eq!(parse_page(Some("3")), Some(3));
        assert_eq!(parse_page(Some(" 2 ")), Some(2));
        assert_eq!(parse_page(None), None);
        assert_eq!(parse_page(Some("")), None);
        assert_eq!(parse_page(Some("oops")), None);
        assert_eq!(parse_page(Some("-1")), None);
    }

    /// A multi-field `?sort=` applies every field: a later field breaks the ties
    /// the earlier ones leave, and flipping it reverses only the tied group.
    #[cfg(feature = "server")]
    #[test]
    fn sort_demo_rows_applies_a_secondary_field_as_the_tiebreaker() {
        use super::{sort_demo_rows, DemoRow};
        let row = |name: &str, role: &str| DemoRow {
            name: name.to_string(),
            role: role.to_string(),
        };
        let mut rows = vec![
            row("Scorpio", "Engineer"),
            row("Aries", "Engineer"),
            row("Libra", "Manager"),
        ];

        sort_demo_rows(&mut rows, "role,name");
        let order: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(order, vec!["Aries", "Scorpio", "Libra"]);

        sort_demo_rows(&mut rows, "role,-name");
        let order: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(order, vec!["Scorpio", "Aries", "Libra"]);
    }

    /// Each gallery snippet must be a verbatim substring of the source file it
    /// cites, so the living contract can never drift from the real components.
    #[test]
    fn snippets_are_exact_copies_of_cited_sources() {
        let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for snippet in SNIPPETS {
            let path = crate_root.join(snippet.source);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            assert!(
                source.contains(snippet.code),
                "snippet for {:?} is not a verbatim substring of {}:\n{}",
                snippet.caption,
                snippet.source,
                snippet.code,
            );
        }
    }

    /// The gallery previews every component the theme ships (bar the chrome
    /// wrappers that only compose others), so a component added to
    /// `crate::components` without a gallery entry is caught here rather than
    /// discovered missing months later.
    #[test]
    fn gallery_source_mentions_every_exported_component() {
        let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let gallery = std::fs::read_to_string(crate_root.join("src/design.rs")).expect("design.rs");
        for component in [
            "AppFooter",
            "AppNavbar",
            "BackBreadcrumb",
            "LawyerPortalBreadcrumb",
            "Card",
            "CodeBlock",
            "ConfirmDelete",
            "DataTable",
            "LegalBlueprintDisclaimer",
            "FormCard",
            "GitHubStars",
            "Icon",
            "ExternalLink",
            "NavigatorShell",
            "Pagination",
            "PeopleListInputs",
            "PricingSection",
            "PublicShell",
            "RowActions",
            "SampleMattersBanner",
            "SiteFooterLegal",
            "SiteHeader",
            "SocialMeta",
            "TestimonialSection",
            "Toast",
        ] {
            assert!(
                gallery.contains(component),
                "the /design gallery does not render {component}"
            );
        }
    }
}
