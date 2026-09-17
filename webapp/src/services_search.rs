//! `/services`' individual-services band, and the search over it.
//!
//! This is the public site's first hydrated surface. Everything else a visitor
//! reads is static markup; this band filters on every keystroke once the
//! WebAssembly bundle has hydrated it.
//!
//! **The GET is the mechanism; the keystroke is the enhancement.** The form is
//! a real `GET /services`, the example chips are real links carrying `?q=`, and
//! the server render filters on the `?q=` it was given. A reader with no
//! JavaScript types, presses Enter, and gets a filtered list back. That is not
//! a courtesy: `portal::dioxus_app::router` mounts the client bundle only when
//! `DIOXUS_PUBLIC_PATH` names a built directory, so a deployment that has not
//! built the bundle serves this page with no hydration at all and the no-JS
//! path is the *normal* path there.
//!
//! Because the URL is written by real navigation rather than by the History
//! API, there is no inline script and nothing to widen the route's CSP for —
//! see the CSP notes in `portal::dioxus_app`. The signal's job is only to
//! narrow the already-rendered list between navigations.
//!
//! The matching rule is ported from the `navigator-ux` gallery specimen so the
//! two renderers agree on what a search finds. See [`search_terms`].

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::marketing_page::BandHeading;

/// The query parameter the search reads and writes.
pub const SEARCH_PARAM: &str = "q";

/// The longest needle the search reads.
///
/// `?q=` is public and unauthenticated, and matching is linear in the needle's
/// term count: every term is scanned against every service. A needle nobody
/// typed — a megabyte of text in a crafted URL — would otherwise buy an
/// unbounded amount of server work per request. A real search is a phrase, so
/// the cap is set well above anything a reader types and the excess is dropped
/// rather than refused: a long URL still renders a page.
pub const MAX_QUERY_LEN: usize = 128;

/// `raw`, cut to at most [`MAX_QUERY_LEN`] characters.
///
/// Cut on a character boundary, not a byte one: a needle ending mid-codepoint
/// would panic on the slice.
#[must_use]
pub fn clamp_query(raw: &str) -> String {
    match raw.char_indices().nth(MAX_QUERY_LEN) {
        Some((index, _)) => raw[..index].to_string(),
        None => raw.to_string(),
    }
}

/// Words dropped from a search before matching.
///
/// A visitor types a sentence — "help with my LLC" — not a keyword. Requiring
/// every word to appear would make that sentence match nothing, so the words
/// that carry no signal are removed first. Ported verbatim from the gallery
/// specimen's `matches`.
pub const STOPWORDS: &[&str] = &[
    "i", "a", "an", "the", "my", "our", "need", "help", "with", "for", "to", "me", "about", "have",
    "want", "am", "is",
];

/// The searchable terms in `needle`.
///
/// Lower-case, split on runs of non-alphanumeric characters, stopwords
/// removed. Splitting on punctuation rather than whitespace is what the
/// specimen does and what makes `llc-file` two terms and `501(c)(3)` three —
/// a reader who types a hyphenated or parenthesised phrase gets the parts
/// matched rather than the literal string missed.
#[must_use]
pub fn search_terms(needle: &str) -> Vec<String> {
    needle
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty() && !STOPWORDS.contains(term))
        .map(str::to_string)
        .collect()
}

/// One service, resolved for rendering.
///
/// Plain data with the fees already resolved: the wasm client filters this
/// list, so it must cross the hydration boundary without needing the catalog's
/// flat-fee lookup or its category vocabulary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Service {
    /// The catalog id, used as the card's anchor.
    pub id: String,
    /// The item code a reader can quote back to the firm.
    pub item: String,
    pub name: String,
    pub blurb: String,
    /// The category's reader-facing label, e.g. `Start a business`.
    pub category: String,
    /// Extra words this service should be findable by that a reader would
    /// never see printed — "personal family legacy" for estate work. Searched,
    /// never rendered.
    pub audience: String,
    /// What the fee buys.
    pub includes: Vec<String>,
    pub keywords: Vec<String>,
    /// The a la carte fee: the service's own amount, or the catalog's flat-fee lookup.
    pub fee: String,
    /// What the fee is charged per.
    pub period: String,
    /// Whether a plan is required.
    pub members_only: bool,
    /// Whether a government body charges its own fee on top.
    pub state_fee: bool,
    /// When set, this service is a Notation package with its included work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<ServicePackageQuote>,
    /// The lower price a named plan pays for this Notation package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_price: Option<PlanPrice>,
    /// The public service id is mapped to a seeded Notation template when a
    /// visitor may start this service online.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// The price a named plan pays for a Notation package.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PlanPrice {
    pub amount: String,
    pub plan: String,
}

/// The included Notations a package publishes.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ServicePackageQuote {
    pub members: Vec<String>,
}

impl Service {
    /// Everything this service can be found by, lower-cased.
    ///
    /// Built once per match rather than per term. The fields are the
    /// specimen's: item number, name, blurb, category label, the invisible
    /// audience words, and the keywords. `includes` is deliberately absent —
    /// the specimen does not search scope lines, and adding them here would
    /// make the two renderers disagree about what a search finds.
    fn haystack(&self) -> String {
        let mut out = String::with_capacity(
            self.item.len() + self.name.len() + self.blurb.len() + self.category.len() + 64,
        );
        for part in [
            self.item.as_str(),
            self.name.as_str(),
            self.blurb.as_str(),
            self.category.as_str(),
            self.audience.as_str(),
        ] {
            out.push_str(part);
            out.push(' ');
        }
        for keyword in &self.keywords {
            out.push_str(keyword);
            out.push(' ');
        }
        out.to_lowercase()
    }

    /// Whether this service matches `needle`.
    ///
    /// Every remaining term must appear, as a substring. An empty needle — or
    /// one that is nothing but stopwords — has no terms, so it matches
    /// everything, which is what an untouched search box should show.
    #[must_use]
    pub fn matches(&self, needle: &str) -> bool {
        let terms = search_terms(needle);
        if terms.is_empty() {
            return true;
        }
        let haystack = self.haystack();
        terms.iter().all(|term| haystack.contains(term))
    }
}

/// One example search offered as a chip.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SearchExample {
    pub label: String,
    pub query: String,
}

/// The whole individual-services band: its heading, the search chrome, and
/// the services themselves.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ServicesBand {
    pub anchor: String,
    pub overline: String,
    pub heading: String,
    pub description: Option<String>,
    /// The search input's label. Visually hidden — the band heading is the
    /// visible one — but a screen reader still needs it.
    pub search_label: String,
    pub search_placeholder: String,
    pub submit_label: String,
    pub examples: Vec<SearchExample>,
    pub fee_label: String,
    pub includes_label: String,
    /// The chip a Notation package carries.
    pub package_badge: String,
    /// The label above a package's included Notations.
    pub package_members_label: String,
    /// The chip a service requiring a plan carries.
    pub members_badge: String,
    /// The chip a service with a government charge carries. This is a
    /// regulated disclosure, not decoration: a reader deciding whether they
    /// can afford a formation needs to know a second bill is coming.
    pub state_fee_badge: String,
    pub empty: String,
    pub empty_help: String,
    pub clear_label: String,
    /// The label and one-line explanation on a service that has a start door.
    #[serde(default)]
    pub start_label: String,
    #[serde(default)]
    pub start_microcopy: String,
    pub services: Vec<Service>,
}

impl ServicesBand {
    /// The services matching `needle`, in publication order.
    #[must_use]
    pub fn matching(&self, needle: &str) -> Vec<&Service> {
        self.services
            .iter()
            .filter(|service| service.matches(needle))
            .collect()
    }

    /// The band's own URL, with `needle` applied. Used by the form, the
    /// example chips, and the "show all" link, so every route back into this
    /// band is spelled the same way.
    #[must_use]
    pub fn href(&self, needle: &str) -> String {
        let anchor = if self.anchor.is_empty() {
            String::new()
        } else {
            format!("#{}", self.anchor)
        };
        if needle.is_empty() {
            format!("{SERVICES_PATH}{anchor}")
        } else {
            format!(
                "{SERVICES_PATH}?{SEARCH_PARAM}={}{anchor}",
                percent_encoding::utf8_percent_encode(needle, percent_encoding::NON_ALPHANUMERIC)
            )
        }
    }
}

/// Where the form submits and the chips link. The band is a section of one
/// page, so this is that page rather than a route of its own.
const SERVICES_PATH: &str = "/services";

/// The individual-services band.
///
/// `query` is the `?q=` the server was given. It seeds the signal, so the
/// client's first render reproduces the server's markup exactly — which is
/// what hydration requires — and the reader can then keep typing from it.
#[component]
pub fn ServicesSearch(band: ServicesBand, query: String) -> Element {
    let mut needle = use_signal(|| query);
    let current = needle();
    let matched = band.matching(&current);
    let total = band.services.len();
    let found = matched.len();
    rsx! {
        section { class: "fm-band fm-band--services", id: "{band.anchor}",
            div { class: "fm-band__inner",
                BandHeading {
                    overline: band.overline.clone(),
                    heading: band.heading.clone(),
                    description: band.description.clone(),
                }
                // The landmark goes on the form, which is what `role="search"`
                // labels — a region of the page, not a control inside it.
                form {
                    class: "fm-services__search",
                    role: "search",
                    method: "get",
                    action: "{SERVICES_PATH}",
                    label {
                        class: "fm-visually-hidden",
                        r#for: "services-search-input",
                        "{band.search_label}"
                    }
                    input {
                        id: "services-search-input",
                        class: "fm-services__input",
                        r#type: "search",
                        name: "{SEARCH_PARAM}",
                        value: "{current}",
                        placeholder: "{band.search_placeholder}",
                        maxlength: "{MAX_QUERY_LEN}",
                        autocomplete: "off",
                        oninput: move |event| needle.set(event.value()),
                    }
                    button {
                        class: "nav-btn nav-btn--primary fm-services__submit",
                        r#type: "submit",
                        "{band.submit_label}"
                    }
                }
                if !band.examples.is_empty() {
                    ul { class: "fm-services__examples",
                        for example in band.examples.iter() {
                            li {
                                a {
                                    class: "fm-services__example",
                                    href: "{band.href(&example.query)}",
                                    "{example.label}"
                                }
                            }
                        }
                    }
                }
                // Announced rather than merely drawn: a filter that changes the
                // list under a screen-reader user without saying so is a list
                // that silently got shorter.
                p { class: "fm-services__count", role: "status", "aria-live": "polite",
                    if found == total {
                        "Showing all {total} services."
                    } else {
                        "Showing {found} of {total} services."
                    }
                }
                if matched.is_empty() {
                    div { class: "fm-services__empty",
                        p { class: "fm-services__empty-heading", "{band.empty}" }
                        p { "{band.empty_help}" }
                        a { class: "fm-card__link", href: "{band.href(\"\")}", "{band.clear_label}" }
                    }
                } else {
                    ul { class: "fm-cards fm-services__list",
                        for service in matched.iter() {
                            li { class: "fm-card fm-services__service", id: "service-{service.id}",
                                h3 { class: "fm-card__title", "{service.name}" }
                                p { class: "fm-services__category", "{service.category}" }
                                div { class: "fm-services__pricing",
                                    p { class: "fm-services__price-choice",
                                        span { class: "fm-services__fee-label", "{band.fee_label}" }
                                        strong { class: "fm-services__fee-amount", "{service.fee}" }
                                        span { class: "fm-services__fee-period", "{service.period}" }
                                    }
                                    if let Some(plan_price) = service.plan_price.as_ref() {
                                        p { class: "fm-services__price-choice fm-services__price-choice--plan",
                                            span { class: "fm-services__fee-label", "With {plan_price.plan}" }
                                            strong { class: "fm-services__fee-amount", "{plan_price.amount}" }
                                            span { class: "fm-services__fee-period", "plan price" }
                                        }
                                    }
                                }
                                if let Some(package) = service.package.as_ref() {
                                    div { class: "fm-services__package",
                                        p { class: "fm-services__package-badge", "{band.package_badge}" }
                                        if !package.members.is_empty() {
                                            p { class: "fm-services__package-members-label", "{band.package_members_label}" }
                                            ul { class: "fm-services__package-members",
                                                for member in package.members.iter() {
                                                    li { "{member}" }
                                                }
                                            }
                                        }
                                    }
                                }
                                if service.members_only || service.state_fee {
                                    ul { class: "fm-chips",
                                        if service.members_only {
                                            li { class: "fm-chip", "{band.members_badge}" }
                                        }
                                        if service.state_fee {
                                            li { class: "fm-chip", "{band.state_fee_badge}" }
                                        }
                                    }
                                }
                                div { class: "fm-card__body",
                                    p { "{service.blurb}" }
                                }
                                if service.template.is_some() {
                                    a {
                                        class: "fm-card__link",
                                        href: "/start/{service.id}",
                                        "{band.start_label}"
                                    }
                                    p { class: "fm-services__start-microcopy", "{band.start_microcopy}" }
                                }
                                if !service.includes.is_empty() {
                                    p { class: "fm-services__includes-label", "{band.includes_label}" }
                                    ul { class: "fm-services__includes",
                                        for line in service.includes.iter() {
                                            li { "{line}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(id: &str, name: &str, blurb: &str, category: &str) -> Service {
        Service {
            id: id.to_string(),
            item: "1101".to_string(),
            name: name.to_string(),
            blurb: blurb.to_string(),
            category: category.to_string(),
            audience: String::new(),
            includes: vec!["A scope line".to_string()],
            keywords: Vec::new(),
            fee: "$50".to_string(),
            period: "per form".to_string(),
            members_only: false,
            state_fee: false,
            package: None,
            plan_price: None,
            template: None,
        }
    }

    fn llc() -> Service {
        let mut service = service(
            "llc-file",
            "Start a company",
            "We set up your limited liability company.",
            "Start a business",
        );
        service.keywords = vec!["LLC".to_string(), "articles of organization".to_string()];
        service
    }

    /// A visitor types a sentence, not a keyword. The stopwords have to go or
    /// the sentence matches nothing.
    #[test]
    fn stopwords_are_dropped() {
        assert_eq!(search_terms("help with my LLC"), vec!["llc"]);
        assert_eq!(search_terms("I need a will"), vec!["will"]);
        assert!(llc().matches("help with my LLC"));
        assert!(llc().matches("llc"));
    }

    /// Every remaining term must appear. One hit out of two is not a match —
    /// otherwise a two-word search widens the results instead of narrowing
    /// them.
    #[test]
    fn every_term_must_appear() {
        assert!(llc().matches("company limited"));
        assert!(!llc().matches("company trademark"));
    }

    /// The needle splits on punctuation, not only whitespace, so a reader who
    /// types a hyphenated or parenthesised phrase gets its parts matched.
    #[test]
    fn the_needle_splits_on_punctuation() {
        assert_eq!(search_terms("llc-file"), vec!["llc", "file"]);
        assert_eq!(search_terms("501(c)(3)"), vec!["501", "c", "3"]);
        assert_eq!(search_terms("  LLC,  formation "), vec!["llc", "formation"]);
    }

    /// A needle nobody typed is cut rather than refused, so a crafted URL
    /// still renders a page and still costs bounded work.
    #[test]
    fn an_overlong_needle_is_cut_to_the_cap() {
        let long = "a".repeat(MAX_QUERY_LEN * 50);
        assert_eq!(clamp_query(&long).chars().count(), MAX_QUERY_LEN);
        // A needle at or under the cap is untouched.
        assert_eq!(clamp_query("llc"), "llc");
        let exact = "b".repeat(MAX_QUERY_LEN);
        assert_eq!(clamp_query(&exact), exact);
    }

    /// Cutting happens on a character boundary. A byte-wise cut through a
    /// multi-byte codepoint panics.
    #[test]
    fn the_cap_cuts_on_a_character_boundary() {
        let multibyte = "é".repeat(MAX_QUERY_LEN * 2);
        let cut = clamp_query(&multibyte);
        assert_eq!(cut.chars().count(), MAX_QUERY_LEN);
        assert!(multibyte.starts_with(&cut));
    }

    /// The rendered input carries the same cap the server applies, so the two
    /// halves of the contract cannot drift.
    #[test]
    fn the_input_publishes_the_same_cap_the_server_applies() {
        let out = render_with("");
        assert!(
            out.contains(&format!(r#"maxlength="{MAX_QUERY_LEN}""#)),
            "{out}"
        );
    }

    /// An untouched search box shows everything, and so does one holding only
    /// stopwords — there is nothing left to filter on.
    #[test]
    fn an_empty_needle_matches_everything() {
        assert!(llc().matches(""));
        assert!(llc().matches("   "));
        assert!(llc().matches("i need help with"));
    }

    /// The category label and the keywords are searchable, not just the name:
    /// a reader who does not know the firm's word for the work still finds it.
    #[test]
    fn the_category_label_and_keywords_are_searchable() {
        assert!(llc().matches("articles of organization"));
        assert!(llc().matches("start a business"));
        // The item number, too — a reader quoting a code back to the firm.
        assert!(llc().matches("1101"));
    }

    /// The invisible audience words are searchable and never printed, which
    /// is what lets "family" find estate work that never uses the word.
    #[test]
    fn the_audience_words_are_searchable() {
        let mut will = service("will", "Make a will", "A lawyer prepares it.", "Wills");
        will.audience = "personal family legacy".to_string();
        assert!(will.matches("family"));
        assert!(
            !service("will", "Make a will", "A lawyer prepares it.", "Wills").matches("family")
        );
    }

    /// Scope lines are deliberately outside the haystack: the specimen does
    /// not search them, and a renderer that did would find different services
    /// for the same words.
    #[test]
    fn scope_lines_are_not_searched() {
        let mut trust = service(
            "trust",
            "Set up a trust",
            "A trust you can change.",
            "Wills",
        );
        trust.includes = vec!["Paperwork to put one Nevada property into the trust".to_string()];
        assert!(!trust.matches("nevada"));
    }

    fn band() -> ServicesBand {
        ServicesBand {
            anchor: "fees".to_string(),
            services: vec![
                llc(),
                service("will", "Make a will", "A lawyer prepares it.", "Wills"),
            ],
            ..ServicesBand::default()
        }
    }

    /// A band with the whole search chrome filled in, for the render tests.
    fn rendered_band() -> ServicesBand {
        let mut trademark = service(
            "trademark",
            "Apply for a trademark",
            "We file your application.",
            "Names and brands",
        );
        trademark.item = "1301".to_string();
        trademark.members_only = true;
        trademark.state_fee = true;
        trademark.period = "+ government filing fees".to_string();
        let mut setup = llc();
        setup.package = Some(ServicePackageQuote {
            members: vec![
                "Start a company".to_string(),
                "An agreement between the owners".to_string(),
                "A federal tax ID for your business".to_string(),
            ],
        });
        setup.plan_price = Some(PlanPrice {
            amount: "$25".to_string(),
            plan: "Business plan".to_string(),
        });
        ServicesBand {
            anchor: "fees".to_string(),
            overline: "Individual services".to_string(),
            heading: "What we can help with".to_string(),
            description: Some("Choose the help you need.".to_string()),
            search_label: "Search individual services".to_string(),
            search_placeholder: "A contract, my business, a will".to_string(),
            submit_label: "Search".to_string(),
            examples: vec![SearchExample {
                label: "My business".to_string(),
                query: "business".to_string(),
            }],
            fee_label: "A la carte price".to_string(),
            includes_label: "What this includes".to_string(),
            package_badge: "Notation package".to_string(),
            package_members_label: "Package includes".to_string(),
            members_badge: "Plan required".to_string(),
            state_fee_badge: "Government fees cost extra".to_string(),
            empty: "We could not find a match.".to_string(),
            empty_help: "Tell us what you need.".to_string(),
            clear_label: "Show all services".to_string(),
            start_label: "Start".to_string(),
            start_microcopy: "Opens a short questionnaire.".to_string(),
            services: vec![setup, trademark],
        }
    }

    fn render_with(query: &str) -> String {
        let query = query.to_string();
        let mut dom = VirtualDom::new_with_props(
            ServicesSearch,
            ServicesSearchProps {
                band: rendered_band(),
                query,
            },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    /// With no `?q=`, the server renders the whole schedule. This is the page
    /// a reader lands on, and the page a reader with no JavaScript keeps.
    #[test]
    fn an_unfiltered_render_lists_every_service() {
        let out = render_with("");
        assert!(out.contains("Start a company"), "{out}");
        assert!(out.contains("Apply for a trademark"), "{out}");
        assert_eq!(
            out.matches("fm-services__service").count(),
            2,
            "every service renders: {out}"
        );
        assert!(
            out.contains("Showing all 2 services."),
            "the count names the whole schedule: {out}"
        );
    }

    /// The server filters on the `?q=` it was given. The keystroke filtering
    /// is the enhancement; this is the mechanism.
    #[test]
    fn a_query_narrows_the_render_on_the_server() {
        let out = render_with("llc");
        assert!(out.contains("Start a company"), "{out}");
        assert!(
            !out.contains("Apply for a trademark"),
            "the unmatched service must not render: {out}"
        );
        assert_eq!(out.matches("fm-services__service").count(), 1, "{out}");
        assert!(out.contains("Showing 1 of 2 services."), "{out}");
        // The input comes back holding what was searched, so the reader can
        // edit it rather than retype it.
        assert!(out.contains(r#"value="llc""#), "{out}");
    }

    /// A search that finds nothing renders an answer, not a blank band. A
    /// band that simply vanished would read as a broken page.
    #[test]
    fn a_no_match_renders_an_empty_state() {
        let out = render_with("bankruptcy");
        assert!(out.contains("fm-services__empty"), "{out}");
        assert!(out.contains("We could not find a match."), "{out}");
        assert!(out.contains("Tell us what you need."), "{out}");
        assert!(
            out.contains("Show all services"),
            "the empty state offers a way back: {out}"
        );
        assert!(
            out.contains(r#"href="/services#fees""#),
            "and that way back is a real link: {out}"
        );
        assert!(!out.contains("fm-services__service"), "{out}");
    }

    /// The form is a real `GET /services`, so a reader with no JavaScript
    /// submits it and gets a filtered page back.
    #[test]
    fn the_form_is_a_get_to_the_services_page() {
        let out = render_with("");
        assert!(out.contains(r#"method="get""#), "{out}");
        assert!(out.contains(r#"action="/services""#), "{out}");
        // The landmark belongs on the form — it labels a region of the page,
        // not a control — and the input is what carries the parameter name.
        assert!(out.contains(r#"role="search""#), "{out}");
        assert!(out.contains(r#"name="q""#), "{out}");
        assert!(out.contains(r#"type="search""#), "{out}");
        // The label is present even though it is visually hidden.
        assert!(out.contains("Search individual services"), "{out}");
        assert!(out.contains(r#"for="services-search-input""#), "{out}");
    }

    /// The example chips are links carrying their own `?q=`, not buttons that
    /// need script to do anything.
    #[test]
    fn the_example_chips_are_links_carrying_a_query() {
        let out = render_with("");
        assert!(out.contains(r#"href="/services?q=business#fees""#), "{out}");
        assert!(out.contains("My business"), "{out}");
    }

    /// The two regulated badges reach the reader from the structured flags,
    /// and only on the services that carry them.
    #[test]
    fn the_regulated_badges_render_from_their_flags() {
        let out = render_with("trademark");
        assert!(out.contains("Plan required"), "{out}");
        assert!(out.contains("Government fees cost extra"), "{out}");
        let unflagged = render_with("llc");
        assert!(!unflagged.contains("Plan required"), "{unflagged}");
        assert!(
            !unflagged.contains("Government fees cost extra"),
            "{unflagged}"
        );
    }

    /// A Notation package makes the a la carte and plan prices easy to compare,
    /// and a service that is not a package does not.
    #[test]
    fn a_package_card_prints_its_plan_price() {
        let packaged = render_with("llc");
        assert!(packaged.contains("Notation package"), "{packaged}");
        assert!(packaged.contains("A la carte price"), "{packaged}");
        assert!(packaged.contains("With Business plan"), "{packaged}");
        assert!(packaged.contains("$25"), "{packaged}");
        assert!(packaged.contains("Package includes"), "{packaged}");
        assert!(
            packaged.contains("An agreement between the owners"),
            "{packaged}"
        );
        let unflagged = render_with("trademark");
        assert!(!unflagged.contains("Notation package"), "{unflagged}");
        assert!(!unflagged.contains("With Business plan"), "{unflagged}");
    }

    /// The result count is announced, so a filter that shortens the list says
    /// so rather than changing it silently under a screen reader.
    #[test]
    fn the_result_count_is_a_live_region() {
        let out = render_with("llc");
        assert!(out.contains(r#"role="status""#), "{out}");
        assert!(out.contains(r#"aria-live="polite""#), "{out}");
    }

    #[test]
    fn matching_filters_in_publication_order() {
        let band = band();
        assert_eq!(band.matching("").len(), 2);
        let narrowed = band.matching("llc");
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].id, "llc-file");
        assert!(band.matching("bankruptcy").is_empty());
    }

    /// Every route back into the band is the same URL, and a needle that
    /// carries a space or an ampersand survives it.
    #[test]
    fn the_band_href_carries_the_needle_and_the_anchor() {
        let band = band();
        assert_eq!(band.href(""), "/services#fees");
        assert_eq!(band.href("llc"), "/services?q=llc#fees");
        assert_eq!(
            band.href("my business & me"),
            "/services?q=my%20business%20%26%20me#fees"
        );
        let unanchored = ServicesBand::default();
        assert_eq!(unanchored.href(""), "/services");
        assert_eq!(unanchored.href("llc"), "/services?q=llc");
    }
}
