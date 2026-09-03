//! The firm's public marketing-page copy, loaded from `locales/en/`.
//!
//! The words live in the English catalog. This module keeps the page
//! constructors the router calls and the advertising guards that read them.
//! Editing published copy is a YAML change; these functions stay as the
//! typed load.

use webapp::marketing_page::PageContent;

use crate::locales;

/// `/fractional-cto` — the firm's lead offering: it runs the technology
/// function for a law firm. See `locales/en/fractional-cto.yaml`.
pub fn fractional_cto() -> PageContent {
    locales::fractional_cto()
}

/// `/navigator` — the platform the firms the firm serves work on. See
/// `locales/en/navigator.yaml`.
pub fn navigator() -> PageContent {
    locales::navigator()
}

/// `/services` — the published flat-fee schedule. See `locales/en/services.yaml`.
pub fn legal_services() -> PageContent {
    locales::legal_services()
}

/// The regulated claims on the firm's public pages.
///
/// `/navigator` and `/services` are the firm's, so the copy and the guards that
/// hold its claims in place live in the binary that publishes them rather than
/// in the application underneath.
#[cfg(test)]
mod firm_copy_tests {
    use webapp::marketing_page::{Band, Paragraph};

    /// Every word of prose a band renders, flattened. Titles, leads, overlines,
    /// descriptions, chips, and card bodies all count: a reader does not
    /// distinguish the struct field a claim arrived in.
    ///
    /// The `overline` and `description` fields are read for exactly that
    /// reason. They were previously skipped, which meant a regulated claim —
    /// a rate, a turnaround promise, a comparative superlative — placed in a
    /// band's description was invisible to every guard in this module while
    /// rendering to the reader like any other sentence. A guard that reads
    /// only some of the page is a guard that reports green on the half it
    /// cannot see.
    fn band_text(band: &Band) -> String {
        fn paragraphs(body: &[Paragraph]) -> String {
            body.iter()
                .flat_map(|p| p.iter().map(|r| r.text.clone()))
                .collect::<Vec<_>>()
                .join(" ")
        }
        match band {
            Band::Statement {
                heading,
                lead,
                body,
            } => format!("{heading} {lead} {}", paragraphs(body)),
            Band::Cards {
                overline,
                heading,
                description,
                items,
                ..
            } => {
                let cards = items
                    .iter()
                    .map(|c| format!("{} {} {}", c.title, c.chips.join(" "), paragraphs(&c.body)))
                    .collect::<Vec<_>>()
                    .join(" ");
                let description = description.clone().unwrap_or_default();
                format!("{overline} {heading} {description} {cards}")
            }
            Band::Steps {
                overline,
                heading,
                description,
                items,
                ..
            } => {
                let steps = items
                    .iter()
                    .map(|s| format!("{} {}", s.title, paragraphs(&s.body)))
                    .collect::<Vec<_>>()
                    .join(" ");
                let description = description.clone().unwrap_or_default();
                format!("{overline} {heading} {description} {steps}")
            }
            Band::ProjectNetwork {
                overline,
                heading,
                description,
                left,
                right,
                mcp_tools,
                agentic_coding_tools,
                saas_tools,
                ..
            } => {
                let nodes = left
                    .iter()
                    .chain(right)
                    .map(|node| format!("{} {}", node.label, node.detail))
                    .collect::<Vec<_>>()
                    .join(" ");
                let description = description.clone().unwrap_or_default();
                format!(
                    "{overline} {heading} {description} {nodes} {} {} {}",
                    mcp_tools.join(" "),
                    agentic_coding_tools.join(" "),
                    saas_tools.join(" ")
                )
            }
            // Every string the band puts on the page: the headings, the
            // version, each box's label, architecture, and filename, and the
            // package-manager prose and commands. The copy guards below read
            // this text, and a download box is as much published copy as a
            // paragraph is — a claim smuggled into a box's `detail` would
            // otherwise never be read by them.
            Band::Downloads {
                overline,
                heading,
                description,
                version,
                archive_label,
                items,
                package,
                ..
            } => {
                let boxes = items
                    .iter()
                    .map(|d| format!("{} {} {}", d.label, d.detail, d.filename))
                    .collect::<Vec<_>>()
                    .join(" ");
                let package = package.as_ref().map_or_else(String::new, |p| {
                    format!(
                        "{} {} {}",
                        p.heading,
                        paragraphs(&p.body),
                        p.commands.join(" ")
                    )
                });
                let description = description.clone().unwrap_or_default();
                format!(
                    "{overline} {heading} {description} {version} {archive_label} {boxes} {package}"
                )
            }
            Band::Cta { heading, body, .. } => {
                format!("{heading} {}", body.clone().unwrap_or_default())
            }
        }
    }

    /// Every destination the page routes a reader to.
    ///
    /// [`band_text`] reads run and card *text* and never an `href`, which is
    /// correct for the copy guards — a URL is not a regulated claim — but it
    /// leaves "does this band actually route anywhere" unassertable. A band can
    /// say "ask us" and link nowhere, and every guard in this module reports
    /// green. So routing is read separately from copy rather than by widening
    /// what the copy guards see.
    fn band_hrefs(bands: &[Band]) -> Vec<String> {
        fn from_paragraphs(body: &[Paragraph]) -> Vec<String> {
            body.iter()
                .flat_map(|p| p.iter().filter_map(|r| r.href.clone()))
                .collect()
        }
        bands
            .iter()
            .flat_map(|band| match band {
                Band::Statement { body, .. } => from_paragraphs(body),
                Band::Cards { items, .. } => items
                    .iter()
                    .flat_map(|c| {
                        from_paragraphs(&c.body)
                            .into_iter()
                            .chain(c.href.clone())
                            .collect::<Vec<_>>()
                    })
                    .collect(),
                Band::Steps { items, .. } => items
                    .iter()
                    .flat_map(|s| from_paragraphs(&s.body))
                    .collect(),
                Band::ProjectNetwork { .. } | Band::Downloads { .. } | Band::Cta { .. } => {
                    Vec::new()
                }
            })
            .collect()
    }

    fn page_text(bands: &[Band]) -> String {
        bands.iter().map(band_text).collect::<Vec<_>>().join(" ")
    }

    /// The fee schedule's cards, resolved from the page rather than restated.
    ///
    /// Every guard below reads the rendered band, so adding a matter without
    /// scoping it — or shipping a placeholder in its price — fails here rather
    /// than passing against a list this file happened to keep in step.
    fn fee_cards(content: &webapp::marketing_page::PageContent) -> &[webapp::marketing_page::Card] {
        content
            .bands
            .iter()
            .find_map(|band| match band {
                Band::Cards { items, .. } => Some(items.as_slice()),
                _ => None,
            })
            .expect("the Legal Services page renders its fee schedule as a card band")
    }

    /// The `/navigator` licence band, resolved from the page.
    ///
    /// Scoping matters more here than it looks. The first version of the routing
    /// assertion below read every href on the page and passed with this band's
    /// link deleted — the working-surface card three bands up also points at
    /// `/contact`, so "the page routes to /contact" was true either way. An
    /// assertion about a band has to read that band.
    fn navigator_licence_band(content: &webapp::marketing_page::PageContent) -> &Band {
        content
            .bands
            .iter()
            .find(|band| {
                matches!(band, Band::Statement { lead, .. }
                    if lead.contains("Nobody needs our permission"))
            })
            .expect("the Navigator page states the licence before it offers an exception to it")
    }

    /// The platform page offers one concrete pro bono co-counsel invitation.
    #[test]
    fn the_navigator_page_invites_pro_bono_co_counsel() {
        let content = super::navigator();
        let text = format!("{} {}", page_text(&content.bands), content.meta_description);
        assert_eq!(
            content.tagline,
            "Agentic lawyering designed to scale and mise-en-place argument prep and human judgment."
        );
        assert!(
            text.contains("Co-Counsel a Pro Bono Case with Us"),
            "the only invitation is pro bono co-counsel: {text}"
        );
        assert!(
            !text.to_lowercase().contains("fractional"),
            "the retired fractional offer must not remain: {text}"
        );
        match content.bands.last() {
            Some(Band::Cta {
                email,
                email_subject,
                ..
            }) => {
                assert_eq!(email, views::brand::firm_email());
                assert_eq!(
                    email_subject.as_deref(),
                    Some("Co-Counseling for Good with AI")
                );
            }
            _ => panic!("the co-counsel invitation must be the page CTA"),
        }
    }

    /// The platform page is not a CTO/CISO or consulting advertisement.
    #[test]
    fn the_navigator_page_removes_the_cto_ciso_offer() {
        let content = super::navigator();
        let text = format!("{} {}", page_text(&content.bands), content.meta_description);
        let words = || text.split(|character: char| !character.is_ascii_alphanumeric());
        assert!(
            !words().any(|word| word.eq_ignore_ascii_case("cto")),
            "no CTO offer reaches the page: {text}"
        );
        assert!(
            !words().any(|word| word.eq_ignore_ascii_case("ciso")),
            "no CISO offer reaches the page: {text}"
        );
        // `law-related service` is the RPC 5.7 term of art, and the licence
        // offer is required to use it — so the ban moves off the phrase and onto
        // the *subject* the retired copy attached it to. Banning the phrase
        // outright would mean this page could never make the one disclosure the
        // rule asks for, which is not what removing a consulting offer was for.
        for retired in ["technology function", "consulting"] {
            assert!(
                !text.to_lowercase().contains(retired),
                "the retired consulting offer must not return to this page \
                 (`{retired}`): {text}"
            );
        }
        assert_eq!(
            text.matches("law-related service").count(),
            text.matches("Licensing software is a law-related service")
                .count(),
            "every `law-related service` on this page must be the licence \
             disclosure; any other use is the consulting characterization \
             coming back: {text}"
        );
        assert!(
            !text.contains("Bring a case") && !text.contains("See it in practice"),
            "the sales-style card grid must not remain: {text}"
        );
        assert!(
            !text.contains("Navigator is the AI system we build")
                && !text.contains("everyone loves vibe-coding"),
            "the retired explanatory copy must not remain: {text}"
        );
    }

    /// The Legal Services page is a schedule of scoped matters.
    ///
    /// This is the shape the fee schedule will be published in, asserted before
    /// the figures land. It replaced a page held to the opposite rule — guarded
    /// against containing a `$` at all, because the firm quoted every
    /// engagement privately — so what matters here is that the structure
    /// survives: a list of named matters, each with the scope its future fee
    /// will buy. A card that lost its scope line would leave a bare price with
    /// no boundary the moment a number arrived beside it.
    #[test]
    fn the_schedule_lists_scoped_matters() {
        let content = super::legal_services();
        let fees = fee_cards(&content);
        assert!(
            fees.len() >= 5,
            "the schedule is the page; {} matters is not a schedule",
            fees.len()
        );
        for card in fees {
            assert!(
                !card.body.is_empty(),
                "{} names no scope, which reads as covering everything",
                card.title
            );
        }
    }

    /// A fee is either published properly or not published at all.
    ///
    /// Every entry is unset today and the firm sets them when it decides them,
    /// so this guards the transition rather than the current state: whatever
    /// appears in that column has to be a real figure. A blank string, a `TBD`,
    /// or a `—` would render as a price tag the reader cannot parse, and a
    /// placeholder shipped by accident is exactly the failure that guard is
    /// for.
    #[test]
    fn any_published_fee_is_a_real_figure() {
        let content = super::legal_services();
        for card in fee_cards(&content) {
            let Some(price) = card.chips.first() else {
                continue;
            };
            assert!(
                price.starts_with('$'),
                "{} publishes {price:?}, which is not a fee",
                card.title
            );
            assert!(
                price.chars().any(|c| c.is_ascii_digit()),
                "{} publishes {price:?}, which carries no amount",
                card.title
            );
        }
        assert!(
            fee_cards(&content).iter().all(|card| card.chips.len() <= 1),
            "a matter carries one fee or none; two prices on one card is not a flat fee"
        );
    }

    /// A fee that depends on a government body's own charge says so.
    ///
    /// The firm cannot control what the Secretary of State, the IRS, or the
    /// USPTO charges, and those change without asking us. A formation priced at
    /// a bare `$700` would be read as the whole cost of forming a company, and
    /// the state's invoice afterwards would land as a surprise charge from a
    /// firm that advertised a flat fee.
    #[test]
    fn a_fee_with_a_pass_through_names_it() {
        let content = super::legal_services();
        for card in fee_cards(&content) {
            let Some(price) = card.chips.first() else {
                continue;
            };
            if price.contains('+') {
                assert!(
                    price.contains("fee"),
                    "{} adds a pass-through without naming it: {price}",
                    card.title
                );
            }
        }
    }

    /// Every matter whose fee depends on a government charge says so in its
    /// scope, whether or not a figure is set yet.
    ///
    /// The pass-through is a property of the work, not of the price, so it can
    /// be stated before the fee is. A reader deciding whether they can afford a
    /// formation needs to know a second bill is coming even on a page that has
    /// not named the first one.
    #[test]
    fn a_matter_with_a_government_charge_discloses_it() {
        let content = super::legal_services();
        let cards = fee_cards(&content);
        for matter in ["LLC formation", "Trademark application"] {
            let card = cards
                .iter()
                .find(|card| card.title == matter)
                .unwrap_or_else(|| panic!("{matter} is on the schedule"));
            let scope: String = card
                .body
                .iter()
                .flat_map(|p| p.iter().map(|r| r.text.clone()))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                scope.contains("fee"),
                "{matter} carries a government charge the scope must disclose: {scope}"
            );
        }
    }

    /// The page states the attorney review the work rests on.
    ///
    /// A priced list of legal documents is the shape a document mill takes, and
    /// the one thing separating this page from one is that a licensed attorney
    /// reads what goes out. That has to be on the page, not only in the footer.
    #[test]
    fn the_legal_services_page_names_attorney_review() {
        let content = super::legal_services();
        let text = format!(
            "{} {} {} {}",
            content.title,
            content.tagline,
            content.meta_description,
            page_text(&content.bands)
        );
        assert!(
            text.to_lowercase().contains("attorney"),
            "the page states the attorney review the work rests on: {text}"
        );
    }

    /// The licence band leads with permission, not with the sale.
    ///
    /// This is the assertion the band was drafted around, and the one worth
    /// having a test for: the order of the argument is load-bearing. A reader
    /// who meets "commercial licence" before what is free concludes that
    /// everything has to be paid for — which is wrong, and wrong in the
    /// direction that turns away a legal aid office or a solo practitioner. So
    /// the free half is stated first in the prose, and the assertion is
    /// positional rather than a check that both phrases appear somewhere.
    ///
    /// **The boundary is checked because vagueness here is the expensive
    /// failure.** BUSL defines no "production use", so a band that says a
    /// licence is needed without saying for what leaves every reader guessing,
    /// and a developer standing in front of a public source tree guesses
    /// permissively. Naming the activity is what makes the offer legible.
    ///
    /// **The conversion is checked because it is what keeps the restriction
    /// honest.** Four years, per version, written into the copy the reader
    /// already holds. If that sentence were softened the page would still read
    /// fine while meaning much less.
    #[test]
    fn the_navigator_page_states_what_is_free_before_what_is_sold() {
        let content = super::navigator();
        let text = page_text(&content.bands);
        let lowered = text.to_lowercase();

        let permission = lowered
            .find("no permission to ask for")
            .expect("the page says plainly what needs no permission");
        let sold = lowered
            .find("commercial licence")
            .expect("the page names what it sells");
        assert!(
            permission < sold,
            "what is free has to be stated before what is sold, or a reader \
             concludes a fork needs to buy one: {text}"
        );

        // The boundary itself. Production use is the whole of what is withheld,
        // so the band has to name the activity rather than the term of art.
        for required in [
            "production use",
            "running it for other people",
            "somebody relies",
        ] {
            assert!(
                lowered.contains(required),
                "the page must state `{required}` — an unnamed boundary is one \
                 every reader resolves in their own favour: {text}"
            );
        }

        // The conversion, which is what bounds the restriction.
        for required in [
            "becomes apache-2.0 four years",
            "restriction ends for it permanently",
            "cannot be withdrawn",
        ] {
            assert!(
                lowered.contains(required),
                "the page must state `{required}` — a restriction with no stated \
                 end reads as permanent: {text}"
            );
        }
    }

    /// The licence band discloses that a software licence is not legal work, and
    /// carries no price.
    ///
    /// Two rules meeting on one band. **RPC 5.7**: selling a licence is a
    /// law-related service, and the reader most likely to assume the
    /// attorney-client protections travel with it is a lawyer buying from a law
    /// firm — so the disclaimer has to be on the page rather than only in the
    /// agreement. This is the one page that carries it: `/fractional-cto` used
    /// to make the same disclosure and no longer does.
    ///
    /// **No price.** A deployment's scope is not knowable in advance, so a
    /// figure here would be a floor dressed as a fee — the same reason
    /// litigation and fractional GC carry none. The band quotes through
    /// `/contact` instead, and this test is what keeps a number from drifting in
    /// later.
    #[test]
    fn the_navigator_licence_offer_discloses_its_nature_and_publishes_no_price() {
        let content = super::navigator();
        let text = page_text(&content.bands);
        let lowered = text.to_lowercase();

        assert!(
            lowered.contains("law-related service rather than legal representation"),
            "the licence offer must say it is not legal representation: {text}"
        );
        assert!(
            lowered.contains("does not make us your counsel"),
            "the licence offer must say plainly that buying it does not engage \
             the firm as counsel: {text}"
        );
        assert!(
            lowered.contains("signed retainer"),
            "the page must say where an attorney-client relationship does begin: {text}"
        );

        let routes = band_hrefs(std::slice::from_ref(navigator_licence_band(&content)));
        assert!(
            routes
                .iter()
                .any(|href| href == "mailto:contact@neonlaw.com"),
            "the licence band must route to the firm inbox, where a licence is \
             scoped and quoted like every other engagement; it routes to \
             {routes:?}"
        );

        // No figure, in any of the shapes one arrives in.
        assert!(
            !text.contains('$'),
            "the licence offer publishes no price: {text}"
        );
        for shape in [
            "per month",
            "per year",
            "per seat",
            "starting at",
            "usd",
            "annually",
        ] {
            assert!(
                !lowered.contains(shape),
                "`{shape}` reads as a price on a page that quotes per \
                 engagement: {text}"
            );
        }
    }

    /// The two quoted practices publish no figure.
    ///
    /// Litigation and fractional GC are quoted per engagement because their
    /// scope is not knowable in advance. The consumer schedule does not license
    /// a number on those pages: a published litigation "price" would be a floor
    /// dressed as a fee, which is the misleading-fee-advertising problem the
    /// flat-fee schedule exists to avoid.
    #[test]
    fn the_services_page_does_not_price_litigation_or_fractional_gc() {
        let content = super::legal_services();
        let fees = fee_cards(&content);
        for quoted in ["litigation", "fractional"] {
            assert!(
                !fees
                    .iter()
                    .any(|card| card.title.to_lowercase().contains(quoted)),
                "{quoted} is quoted per engagement and must not appear in the fee schedule"
            );
        }
    }
    /// The page states the vibe-coding thesis and names the interfaces it rests
    /// on.
    ///
    /// Vibe coding is the page's argument, not a garnish on it: it is modern
    /// storytelling, a story is written in passes, and version control is what
    /// makes each pass cost the change rather than the whole document. The named
    /// interfaces are the reason that holds, so dropping either the thesis or the
    /// names leaves the page asserting a preference with nothing under it.
    #[test]
    fn the_navigator_page_makes_the_vibe_coding_case_for_lawyers() {
        let content = super::navigator();
        let text = format!(
            "{} {} {}",
            content.tagline,
            page_text(&content.bands),
            content.meta_description
        );
        assert!(
            text.contains("Vibe coding"),
            "the page keeps the term of art: {text}"
        );
        for named in ["Claude Code", "Codex"] {
            assert!(
                text.contains(named),
                "the page names {named}, the interface the method rests on: {text}"
            );
        }
        // Version control is the mechanism the argument rests on, so the page
        // has to name it rather than gesture at "efficiency".
        assert!(
            text.contains("version control"),
            "the page names the mechanism: {text}"
        );
        assert!(
            text.to_lowercase().contains("storytelling"),
            "the page ties the method to lawyering as storytelling: {text}"
        );
    }

    /// The connected-Project diagram names the Project's work surfaces.
    #[test]
    fn the_navigator_page_maps_connected_project_surfaces() {
        let content = super::navigator();
        let diagram = content
            .bands
            .iter()
            .find_map(|band| match band {
                Band::ProjectNetwork {
                    left,
                    right,
                    mcp_tools,
                    agentic_coding_tools,
                    saas_tools,
                    ..
                } => Some((left, right, mcp_tools, agentic_coding_tools, saas_tools)),
                _ => None,
            })
            .expect("the Navigator page renders its connected-Project diagram");

        let left_labels: Vec<&str> = diagram.0.iter().map(|node| node.label.as_str()).collect();
        assert_eq!(
            left_labels,
            [
                "Internal Slack",
                "Internal Notion",
                "GitHub",
                "Client portal"
            ]
        );
        let right_labels: Vec<&str> = diagram.1.iter().map(|node| node.label.as_str()).collect();
        assert_eq!(
            right_labels,
            [
                "Shared Slack",
                "Per-Project Inbox",
                "Google Drive folder",
                "Shared Notion"
            ]
        );
        assert!(diagram.0[2].detail.contains("Per-project versioned text"));
        assert_eq!(diagram.1[2].detail, "Large document intake");
        assert_eq!(
            diagram.1[3].detail,
            "Client collaboration when the Project uses it."
        );
        let mcp_tools: Vec<&str> = diagram.2.iter().map(String::as_str).collect();
        assert_eq!(mcp_tools, ["Court Listener", "Descrybe", "Exa", "Midpage"]);
        let agentic_coding_tools: Vec<&str> = diagram.3.iter().map(String::as_str).collect();
        assert_eq!(
            agentic_coding_tools,
            ["Antigravity", "Claude Code", "Codex", "Cursor"]
        );
        let saas_tools: Vec<&str> = diagram.4.iter().map(String::as_str).collect();
        assert_eq!(
            saas_tools,
            [
                "Chatwoot",
                "Descript",
                "DocuSign",
                "Google Workspace",
                "Highlight",
                "Linear",
                "Mercury",
                "Restate",
                "SurrealDB",
                "Twilio",
                "Xero"
            ]
        );
    }

    /// The vibe-coding case is argued without a claim the firm cannot
    /// substantiate.
    ///
    /// This is the guard the drafting of that section needed. The thesis it came
    /// from called version control "the most token-efficient way to write legal
    /// documents over time" — a superlative no one can defend under RPC 7.1, on
    /// a page that is lawyer advertising in California, Nevada, and Washington.
    /// The page makes the substantiable claim instead: a revision costs the
    /// change rather than the whole document.
    ///
    /// The banned list is a floor, not the whole rule. A superlative that is not
    /// spelled here is still a superlative.
    #[test]
    fn the_navigator_page_publishes_no_superlative_and_no_turnaround_promise() {
        let content = super::navigator();
        let text = format!(
            "{} {} {}",
            content.tagline,
            page_text(&content.bands),
            content.meta_description
        );
        let lowered = text.to_lowercase();
        for banned in [
            "most token-efficient",
            "fastest",
            "cheapest",
            "world-class",
            "cutting-edge",
            "industry-leading",
            "best-in-class",
            "premier",
            "guarantee",
            "certified",
        ] {
            assert!(
                !lowered.contains(banned),
                "the platform page must not publish {banned:?}: {text}"
            );
        }
        // A turnaround on this page would be a service commitment about work the
        // platform does not do — the retired fractional-GC page was where a
        // redline turnaround belonged, and it is gone.
        assert!(
            !lowered.contains("business day") && !lowered.contains("turnaround"),
            "the platform page promises no turnaround: {text}"
        );
    }

    /// Every card on the working-surface band names something that ships.
    ///
    /// The band exists to tell a firm what it would actually touch, so a card
    /// for a roadmap surface is an advertisement for something the reader cannot
    /// use. AIDA's tool catalog (`mcp/src/tools/`, exposed over MCP and A2A),
    /// the notation templates, and the matter dashboard are all in the tree.
    #[test]
    fn the_working_surface_band_names_three_shipped_surfaces() {
        let content = super::navigator();
        let cards = content
            .bands
            .iter()
            .find_map(|band| match band {
                Band::Cards {
                    overline, items, ..
                } if overline == "The working surface" => Some(items.as_slice()),
                _ => None,
            })
            .expect("the platform page renders its working surface as a card band");
        assert_eq!(cards.len(), 3, "three surfaces, one card each");
        for card in cards {
            assert!(
                !card.body.is_empty(),
                "{} names no work a lawyer does with it",
                card.title
            );
        }
        let titles: Vec<&str> = cards.iter().map(|card| card.title.as_str()).collect();
        assert!(
            titles.iter().any(|title| title.contains("AIDA")),
            "the tool catalog is on the page: {titles:?}"
        );
        assert!(
            titles.iter().any(|title| title.contains("Notation")),
            "the notation templates are on the page: {titles:?}"
        );
    }

    /// `/navigator` groups the operator manuals next to the download band.
    ///
    /// The alphabetical `/docs` catalog remains the full map. This band is four
    /// links a reader who came for the CLI can follow without hunting.
    #[test]
    fn the_read_next_band_links_the_operator_manuals() {
        let content = super::navigator();
        let cards = content
            .bands
            .iter()
            .find_map(|band| match band {
                Band::Cards {
                    overline, items, ..
                } if overline == "Read next" => Some(items.as_slice()),
                _ => None,
            })
            .expect("the platform page groups operator manuals as a card band");
        let hrefs: Vec<&str> = cards
            .iter()
            .filter_map(|card| card.href.as_deref())
            .collect();
        for required in [
            "/docs/validate",
            "https://github.com/neon-law-source-code/navigator/blob/main/docs/gitops.md",
            "/docs/oss-install",
            "/workshops",
        ] {
            assert!(
                hrefs.contains(&required),
                "the read-next band must link {required}: {hrefs:?}"
            );
        }
        assert_eq!(hrefs.len(), 4, "four manuals, four links: {hrefs:?}");
        assert!(
            cards
                .iter()
                .all(|card| card.href.is_some() && card.href_label.is_some()),
            "each card is a link, not a dead tile"
        );
    }
}
