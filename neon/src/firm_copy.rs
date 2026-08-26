//! The firm's public marketing copy.
//!
//! Neon Law's own words about its practice, owned by the binary that publishes
//! them rather than by the application underneath.

/// The firm's public page copy.
///
/// The firm builds Navigator for its own practice. The `/navigator` page makes
/// one invitation: co-counsel a pro bono case with us.
///
/// [`legal_services`] is the priced one: the routine consumer work — a will, a
/// trust, a name change, a formation — with the firm's actual fee printed
/// beside each entry. That is the page's whole reason to exist. A person
/// deciding whether they can afford a lawyer gets an answer from the page
/// rather than from a consultation, and a firm that publishes its fees cannot
/// quietly charge one client more than another for the same work.
use webapp::marketing_page::{
    Band, Card, Download, HeroCta, PackageInstall, PageContent, ProjectNetworkNode, Run, Step,
};

/// One published flat fee: what the matter is, what it costs, and what that
/// figure does and does not cover.
///
/// The scope is not decoration. A fee shown bare reads as "everything this
/// matter could need", so a filing fee billed separately afterwards arrives
/// as a surprise charge from a firm that advertised a fixed price. Every
/// entry states its own boundary, and the footer disclaimer states the
/// general one.
struct FlatFee {
    matter: &'static str,
    /// The published fee, already formatted for the page, or `None` while
    /// the firm has not set one.
    ///
    /// A string rather than a number because some entries carry a
    /// qualifier a number cannot ("+ state fee"), and a price a reader
    /// sees must be the exact string the firm chose rather than something
    /// a formatter derived.
    ///
    /// `None` renders no chip at all rather than "contact us" or a dash. A
    /// placeholder in a price column reads as a price the reader failed to
    /// understand; an absent one reads as absent. Every entry is `None`
    /// today: the schedule's shape is settled and its figures are a
    /// decision for the firm, not a value to be invented here.
    fee: Option<&'static str>,
    scope: &'static str,
}

/// The firm's published fee schedule.
///
/// Ordered as a person meets these matters rather than by price: the
/// estate documents first, because that is the largest share of what
/// walks in; then the personal filings; then the small-business work.
///
/// **Every figure here is a published commitment.** Setting one tells the
/// public what the firm charges, and a fee a client has read cannot be
/// quietly revised upward for them — so a number here is a decision by the
/// firm, never a copy edit and never a placeholder someone forgot to
/// replace. That is why they are all `None` today.
///
/// When they are set: third-party fees — the Secretary of State's, the
/// IRS's, the USPTO's, the court's — are never folded into a figure here.
/// They are set by someone else and change without asking us, so a number
/// that silently included one would go wrong on its own. Write those as
/// `$X + state fee`, which `a_fee_with_a_pass_through_names_it` enforces.
const FLAT_FEES: &[FlatFee] = &[
    FlatFee {
        matter: "Simple will",
        fee: None,
        scope: "One will, drafted from your answers and reviewed by a licensed attorney, \
                through signing and witnessing.",
    },
    FlatFee {
        matter: "Estate package",
        fee: None,
        scope: "A will, a financial power of attorney, and a healthcare directive, drafted \
                together so they agree with one another.",
    },
    FlatFee {
        matter: "Revocable living trust",
        fee: None,
        scope: "The trust, a pour-over will, and the deed transferring one Nevada property \
                into it. Further properties are quoted.",
    },
    FlatFee {
        matter: "Uncontested name change",
        fee: None,
        scope: "The petition, the publication notice, and the hearing. Court filing and \
                publication costs are billed at cost.",
    },
    FlatFee {
        matter: "Demand letter",
        fee: None,
        scope: "One letter over the firm's signature, after we read what you have. It is \
                not a retainer to litigate if the letter does not work.",
    },
    FlatFee {
        matter: "Tenant eviction defense",
        fee: None,
        scope: "The answer and one hearing in a Nevada summary eviction. An appeal or a \
                contested trial is a separate engagement.",
    },
    FlatFee {
        matter: "LLC formation",
        fee: None,
        scope: "Articles, an operating agreement, the EIN, and the initial state filing. \
                The Secretary of State sets its own fee.",
    },
    FlatFee {
        matter: "Nonprofit formation and 501(c)(3)",
        fee: None,
        scope: "Articles, bylaws, a conflict-of-interest policy, and the Form 1023 \
                application. The IRS sets its own user fee.",
    },
    FlatFee {
        matter: "Trademark application",
        fee: None,
        scope: "A clearance search and one class of one application, through filing. The \
                USPTO sets its own per-class fee.",
    },
    FlatFee {
        matter: "Nevada annual report",
        fee: None,
        scope: "The annual list and the state business licence renewal for one entity. The \
                state sets its own fee.",
    },
    FlatFee {
        matter: "Mutual NDA review",
        fee: None,
        scope: "One agreement read and redlined, with a short note on what we changed and \
                why.",
    },
];

/// `/fractional-cto` — the firm's lead offering: it runs the technology
/// function for a law firm.
///
/// **This page advertises a law-related service, and that is what makes it the
/// most constrained page on the host.** Under RPC 5.7 in every jurisdiction the
/// firm advertises in (CA, NV, WA), where a law firm provides non-legal services
/// to a client, the protections of the attorney-client relationship are presumed
/// to apply unless the firm takes reasonable measures to explain that they do
/// not. So the closing band is a disclosure rather than a sales line, and
/// `the_fractional_cto_page_discloses_the_law_related_service_boundary` is what
/// keeps it there.
///
/// Two more constraints the cards are written against. Nothing here claims an
/// attestation — the firm is readiness counsel beside the auditor and never the
/// attester, so "readiness" appears and "certification" cannot. And nothing here
/// publishes a turnaround or a fee: the scope of running a firm's technology
/// function is not knowable in advance, so it is quoted through `/contact` like
/// the litigation and fractional-GC practices.
/// Split one line of a hero statement into words, marking the first
/// `accent_words` of them as the run the firm sets in its own colour.
///
/// The same shape `/litigation` uses: the opening in brand, the claim after it
/// in text. Splitting here rather than in CSS is what lets the copy decide where
/// the emphasis falls, and one call per line is what lets it decide where the
/// statement breaks.
fn hero_line(statement: &str, accent_words: usize) -> Vec<webapp::litigation_page::HeroWord> {
    statement
        .split_whitespace()
        .enumerate()
        .map(|(index, text)| webapp::litigation_page::HeroWord {
            text: text.to_string(),
            accent: index < accent_words,
        })
        .collect()
}

pub fn fractional_cto() -> PageContent {
    PageContent {
        head_title: format!("Fractional CTO — {}", views::brand::FIRM_BRAND.site_name),
        meta_description: "Fractional CTO for law firms — AI enablement delivered through the \
                           firm, with the privacy and compliance work, complex counsel, and a \
                           co-counsel network on Navigator."
            .to_string(),
        title: "Fractional CTO".to_string(),
        hero_mark: Some(webapp::components::PracticeMark::Technology),
        tagline: "Save Time. Serve More.".to_string(),
        // Two lines: the ask in the firm's colour, the promise under it in text.
        hero_lines: vec![hero_line("Save Time.", 2), hero_line("Serve More.", 0)],
        hero_lead: "We run the technology function for the law firms we serve, and we practise \
                    beside them."
            .to_string(),
        hero_cta: Some(HeroCta {
            href: "/contact".to_string(),
            label: "Contact us".to_string(),
        }),
        skin: webapp::marketing_page::PageSkin::Practice,
        bands: vec![
            fractional_cto_intro_band(),
            Band::Cta {
                heading: "Tell us what your firm is trying to do".to_string(),
                body: Some(
                    "Write to us with what your practice runs on today and what you want it to \
                     do. We answer with the scope we would take on and a quote for it."
                        .to_string(),
                ),
                email: views::brand::firm_email().to_string(),
                email_subject: Some("Fractional CTO".to_string()),
            },
        ],
    }
}

/// The engagements, in the firm's own words.
///
/// **This is the copy the home page used to open on**, moved here when the site
/// began leading with the litigation practice instead. Four paragraphs: what
/// vibe coding buys a lawyer, what we configure and deploy, what Navigator does
/// and does not see, and the co-counsel half.
///
/// The named third parties are named deliberately: a firm evaluating this wants
/// to know whether we work with the tools it already runs, and a list is a
/// factual statement about what we configure rather than a claim about outcomes.
/// The Navigator mention links its own page rather than repeating that page here.
fn fractional_cto_intro_band() -> Band {
    Band::Statement {
        heading: "Our engagements".to_string(),
        // No visible lead: the statement that used to sit above this card in
        // large type is now its opening paragraph, so the whole of the copy
        // reads as one block of prose at one size.
        lead: String::new(),
        body: vec![
            vec![Run::plain(
                "We leverage our litigation, transactional, and FAANG-engineering experience to \
                 enhance legal practices with state-of-the-art agentic tooling. We help all \
                 lawyers and clerks tell wonderful stories with vibe-coding that align to their \
                 clients' needs.",
            )],
            vec![Run::plain(
                "We believe vibe coding is an incredibly powerful storytelling tool that allows \
                 you to connect on a deeper level of understanding with your clients. Using \
                 state-of-the-art frontier models, you can create dynamic worlds that are unique \
                 and bespoke to the unique client needs, such as litigation or an estate plan. We \
                 empower you with a safety harness to build these worlds responsibly.",
            )],
            vec![
                Run::plain(
                    "We configure your technical architecture, common software as a service tools \
                     such as Google Workspace, DocuSign, and Xero, AI tooling like Claude and \
                     OpenAI, MCP servers like Descrybe, Midpage, and Trellis, and deploy ",
                ),
                Run::link("Neon Law Navigator", "/navigator"),
                Run::plain(" securely in your environment."),
            ],
            vec![Run::plain(
                "Neon Law Navigator is designed with privacy disclosure and professional ethics \
                 in mind. By default, we do not see our clients' matters. We only collect \
                 anonymized telemetry to ensure your systems are still working.",
            )],
            vec![Run::plain(
                "That being said, our partner firms tap into our litigation and transactional \
                 experience routinely to co-counsel on matters. We work fast, diligently, and \
                 cost-effectively.",
            )],
        ],
    }
}

/// `/navigator` — the platform the firms the firm serves work on: why we build
/// it, why vibe coding is modern storytelling, what a firm works with, and the
/// co-counsel invitation.
pub fn navigator() -> PageContent {
    PageContent {
        skin: webapp::marketing_page::PageSkin::Marketing,
        head_title: format!(
            "Neon Law Navigator — {}",
            views::brand::FIRM_BRAND.site_name
        ),
        meta_description: "Neon Law Navigator is the legal project platform the law firms we \
                           serve work on — vibe coding for lawyers, where every pass at a \
                           document is a change you can read. Source-available under BUSL-1.1, \
                           free for everything but running it for your own clients."
            .to_string(),
        title: "Neon Law Navigator".to_string(),
        // The marketing skin renders the title as the headline and the tagline
        // under it, so the practice skin's accent statement, hero lead, and hero
        // button are not its shape.
        hero_lines: Vec::new(),
        hero_lead: String::new(),
        hero_cta: None,
        hero_mark: Some(webapp::components::PracticeMark::Helm),
        tagline: "Vibe coding for lawyers. Lawyering is storytelling, and a story is written in \
                  passes."
            .to_string(),
        bands: vec![
            navigator_purpose_band(),
            navigator_project_network_band(),
            navigator_vibe_band(),
            navigator_downloads_band(),
            navigator_working_surface_band(),
            navigator_licence_band(),
            Band::Cta {
                heading: "Co-Counsel a Pro Bono Case with Us".to_string(),
                body: Some(
                    "To see how vibe coding can help you tell more persuasive stories, we \
                     invite you to help make the world a better place and explore AI together."
                        .to_string(),
                ),
                email: views::brand::firm_email().to_string(),
                email_subject: Some("Co-Counseling for Good with AI".to_string()),
            },
        ],
    }
}

/// The work surfaces a Project connects around Navigator.
fn navigator_project_network_band() -> Band {
    Band::ProjectNetwork {
        anchor: "connected-project".to_string(),
        overline: "One connected Project".to_string(),
        heading: "Navigator is the center of the work.".to_string(),
        description: Some(
            "A Project can include one or more cases, companies, filings, and more so long as it's related to the best interest of our clients."
                .to_string(),
        ),
        left: vec![
            ProjectNetworkNode {
                label: "Internal Slack".to_string(),
                detail: "Firm-only Project conversation.".to_string(),
            },
            ProjectNetworkNode {
                label: "Internal Notion".to_string(),
                detail: "Firm working notes and knowledge.".to_string(),
            },
            ProjectNetworkNode {
                label: "GitHub".to_string(),
                detail: "Per-project versioned text including notation templates and client portal."
                    .to_string(),
            },
            ProjectNetworkNode {
                label: "Client portal".to_string(),
                detail: "A vibe-coded Project application for the client experience.".to_string(),
            },
        ],
        right: vec![
            ProjectNetworkNode {
                label: "Shared Slack".to_string(),
                detail: "Client collaboration when the Project uses it.".to_string(),
            },
            ProjectNetworkNode {
                label: "Per-Project Inbox".to_string(),
                detail: "Project email intake and conversation.".to_string(),
            },
            ProjectNetworkNode {
                label: "Google Drive folder".to_string(),
                detail: "Large document intake".to_string(),
            },
            ProjectNetworkNode {
                label: "Shared Notion".to_string(),
                detail: "Client collaboration when the Project uses it.".to_string(),
            },
        ],
        mcp_tools: vec![
            "Court Listener".to_string(),
            "Descrybe".to_string(),
            "Exa".to_string(),
            "Midpage".to_string(),
        ],
        agentic_coding_tools: vec![
            "Antigravity".to_string(),
            "Claude Code".to_string(),
            "Codex".to_string(),
            "Cursor".to_string(),
        ],
        saas_tools: vec![
            "Chatwoot".to_string(),
            "Descript".to_string(),
            "DocuSign".to_string(),
            "Google Workspace".to_string(),
            "Highlight".to_string(),
            "Linear".to_string(),
            "Mercury".to_string(),
            "Twilio".to_string(),
            "Xero".to_string(),
        ],
    }
}

/// The firm's client-serving purpose for Navigator, and who works on it.
fn navigator_purpose_band() -> Band {
    Band::Statement {
        heading: "Why we build it".to_string(),
        lead: "We build Navigator for the purpose of serving clients as expeditiously, \
               precisely, accurately, and in alignment with their interests."
            .to_string(),
        body: Vec::new(),
    }
}

/// Vibe coding as the firm's method, and why the interface is Claude Code and
/// Codex rather than a chat window.
///
/// **The claims here are about how the firm works, not about a feature a client
/// firm switches on.** That distinction is load-bearing: this is an
/// attorney-advertising page, so a sentence in the present tense reads as a
/// shipped promise. What is asserted is the method (passes, diffs, review) and
/// the plain mechanics of a diff — never a turnaround, an outcome, or a
/// capability the platform has not shipped.
///
/// No superlative on the token economics either. "The most token-efficient way
/// to write a legal document" is not a claim anyone can substantiate, so the
/// page says the substantiable thing instead: a revision costs the change
/// rather than the whole document.
fn navigator_vibe_band() -> Band {
    Band::Statement {
        heading: "Vibe coding is modern storytelling".to_string(),
        // `Band::Statement` renders its heading screen-reader-only and its lead
        // as the band's display line, so the lead has to be a line and not a
        // paragraph. The argument goes in the body underneath it.
        lead: "Vibe coding is modern storytelling.".to_string(),
        body: vec![
            vec![Run::plain(
                "A brief, a contract, a filing: none of them arrives in one pass. You draft, you \
                 read it back, you change what is not yet true, and you keep the version you \
                 meant. Vibe coding is that same loop with VCS or version control, that makes your \
                 inferences token-efficient. Navigator is built with the premise that it\u{2019}s \
                 more efficient to use Claude Code and Codex rather than a chat window.",
            )],
            // The invitation carries its own links. A page that tells a reader to
            // join a workshop or read the source and then hands them neither is a
            // call to action with nowhere to go.
            vec![
                Run::plain(
                    "Lawyers already know about markdown thanks to LLMs, and we think that \
                     writing code is now just as approachable. We invite you to join us for a ",
                ),
                Run::link("workshop", "/workshops"),
                Run::plain(" or check out our "),
                Run::link(
                    "source code",
                    "https://github.com/neon-law-source-code/navigator",
                ),
                Run::plain(" to learn more."),
            ],
        ],
    }
}

/// The CLI download boxes: Linux, macOS, Windows, and the Homebrew route.
///
/// **The page can publish these because the source is public.** Every release
/// attaches its archives to a public GitHub Release, so this band links those
/// bytes rather than proxying them — no login, no signed URL, no bucket. That is
/// unchanged by the licence: BUSL restricts production *use*, not distribution,
/// so the archives stay downloadable by anyone. The role-gated `/app/team` page is unaffected and
/// still serves the deployment's own private copies to firm tiers; this is a
/// second, public door to the same software.
///
/// The version is the deployment's own release, and every href carries it, so
/// what this page offers is exactly the release the reader is looking at rather
/// than whatever GitHub currently calls latest. `deploy.yml` refuses any tag
/// that does not equal `[workspace.package].version`, which is what makes the
/// environment variable and the manifest the same string.
///
/// **The macOS box names Homebrew for a reason that is not convenience.** The
/// released Mach-O is unsigned and unnotarized. A browser download carries
/// `com.apple.quarantine` and Gatekeeper refuses it outright, while `brew`
/// fetches with `curl`, which sets no such attribute — the same bytes run. So
/// the band recommends the tap rather than leaving a reader to discover this
/// from a binary macOS will not open. Signing is the real fix and is not shipped;
/// nothing here claims otherwise.
fn navigator_downloads_band() -> Band {
    let version = webapp::cli_release::release_version();
    Band::Downloads {
        anchor: "download".to_string(),
        overline: "Download".to_string(),
        heading: "Run Navigator yourself".to_string(),
        description: Some(
            "Navigator is source-available under the Business Source License 1.1 — free to read, \
             build, and use outside production. The command-line tool is how you drive it: pick \
             your platform, or install it with Homebrew."
                .to_string(),
        ),
        version: version.clone(),
        archive_href: webapp::cli_release::RELEASES_HREF.to_string(),
        archive_label: "every release".to_string(),
        items: webapp::cli_release::PLATFORMS
            .iter()
            .map(|platform| Download {
                platform: platform.slug.to_string(),
                label: platform.label.to_string(),
                detail: platform.detail.to_string(),
                filename: webapp::cli_release::asset_filename(&version, platform),
                href: webapp::cli_release::asset_href(&version, platform),
                mark: platform.mark,
            })
            .collect(),
        package: Some(PackageInstall {
            heading: "Install with Homebrew".to_string(),
            body: vec![
                vec![
                    Run::plain("On a Mac this is the route we recommend. The released binary is "),
                    Run::strong("not yet signed or notarized"),
                    Run::plain(
                        ", and macOS refuses a binary downloaded through a browser; Homebrew \
                         fetches the same bytes without that mark, so they run. Homebrew also \
                         works on Linux.",
                    ),
                ],
                vec![
                    Run::plain("The formula lives in our own tap, "),
                    Run::link(
                        "neon-law-source-code/homebrew-navigator",
                        "https://github.com/neon-law-source-code/homebrew-navigator",
                    ),
                    Run::plain(", which is updated by every release."),
                ],
            ],
            commands: vec![
                webapp::cli_release::HOMEBREW_INSTALL_COMMAND.to_string(),
                webapp::cli_release::HOMEBREW_UPGRADE_COMMAND.to_string(),
            ],
        }),
    }
}

/// What a firm actually works with today.
///
/// Every card names something in the tree: the AIDA tool catalog exposed over
/// MCP and A2A, the notation templates, and the portal. Nothing here describes
/// a roadmap item — a card for an unshipped surface is an advertisement for
/// something a firm cannot use.
fn navigator_working_surface_band() -> Band {
    Band::Cards {
        anchor: "surface".to_string(),
        overline: "The working surface".to_string(),
        heading: "What a firm works with".to_string(),
        description: Some(
            "One platform, three things a lawyer touches. Every engagement is quoted through \
             the contact page."
                .to_string(),
        ),
        items: vec![
            Card {
                title: "AIDA and its tools".to_string(),
                chips: vec!["MCP".to_string(), "A2A".to_string()],
                body: vec![vec![Run::plain(
                    "One catalog of tools over open protocols, so the agent a lawyer already \
                     works in can read a matter, answer a questionnaire, or open a project \
                     without leaving the editor.",
                )]],
                href: None,
                href_label: None,
            },
            Card {
                title: "Notation templates".to_string(),
                chips: vec!["Typst".to_string()],
                body: vec![vec![Run::plain(
                    "The documents a matter produces are templates with the answers filled in, \
                     so what the firm files is the template it reviewed rather than a fresh \
                     draft each time.",
                )]],
                href: None,
                href_label: None,
            },
            Card {
                title: "The matter dashboard".to_string(),
                chips: vec![],
                body: vec![vec![Run::plain(
                    "The projects, the people on them, the documents, and the questionnaires \
                     still open — the shared view a firm and its co-counsel read the matter from.",
                )]],
                href: Some("/contact".to_string()),
                href_label: Some("Ask us for a walkthrough".to_string()),
            },
        ],
    }
}

/// The licence, in the order a reader needs it: nobody needs permission, the
/// grant cannot be withdrawn, and only then the narrow thing on offer.
///
/// **The order is the design.** A page that opens on "commercial licences
/// available" reads as though everything needs paying for, and the reader most
/// damaged by that impression is the one this software exists to reach — a legal
/// aid office, a solo practitioner, a firm evaluating whether any of this is
/// worth their afternoon. So what is free comes first, plainly, and the offer
/// comes last.
///
/// **The boundary sentence is the one doing real work, and it changed.** This
/// band said "nobody needs our permission to run Navigator" while the tree was
/// `AGPL-3.0-only`, and that was true. Under `BUSL-1.1` it is true of everything
/// except production use, so the band has to draw the line rather than gesture at
/// it: reading, building, forking, and evaluating are free; running it to deliver
/// legal services to other people is what we sell. Leaving the old sentence up
/// would be the most expensive kind of stale copy — a licensing promise, on a law
/// firm's own site, that the licence in the repository does not make.
///
/// **The conversion is stated because it is the part a reader will not assume.**
/// Every published version becomes `AGPL-3.0-only` four years on, and that is a
/// term of the licence each copy already carries rather than a promise about our
/// future intentions. A reader who has watched a project relicense and strand its
/// users is owed the enforceable version, not the reassuring one.
///
/// **The disclosure is required rather than decorative.** Selling a software
/// licence is a law-related service under RPC 5.7, and the reader most likely
/// to assume otherwise is a lawyer buying it from a law firm.
///
/// Like [`navigator_working_surface_band`], this asserts no capability that has
/// not shipped. No price appears here: the scope of a deployment is not knowable
/// in advance, so it is quoted through `/contact` like every other engagement.
fn navigator_licence_band() -> Band {
    Band::Statement {
        heading: "The licence, and the one thing we sell around it".to_string(),
        lead: "Read it, build it, fork it. Nobody needs our permission for any of that."
            .to_string(),
        body: vec![
            vec![Run::plain(
                "Navigator is source-available under the Business Source License 1.1, over the \
                 whole tree — the code, the tooling, and the drafted legal prose. Read it, build \
                 it, run the tests, stand it up on your own machine, fork it, change it, teach \
                 from it. There is no permission to ask for and nobody to ask.",
            )],
            vec![
                Run::plain("The one thing we sell is "),
                Run::strong("running it for other people"),
                Run::plain(
                    " — operating a portal, a matter, or a filing pipeline that somebody relies \
                     on. That is production use, and the licence does not grant it, so a firm \
                     delivering legal services on Navigator takes a commercial licence from us \
                     first. Everything short of that is already yours.",
                ),
            ],
            vec![
                Run::plain("This does not last. "),
                Run::strong("Every version becomes AGPL-3.0-only four years after we publish it"),
                Run::plain(
                    " — per version, on its own clock, written into the licence that copy \
                     already carries rather than promised separately. When a version converts, \
                     the restriction ends for it permanently and anyone may run it in \
                     production. Copies we distributed under the AGPL before this licence stay \
                     AGPL forever; a licence already granted cannot be withdrawn.",
                ),
            ],
            vec![Run::plain(
                "Licensing software is a law-related service rather than legal representation. \
                 Taking a licence from us does not make us your counsel, and the protections of \
                 the attorney-client relationship — privilege and confidentiality among them — \
                 do not attach to it. An attorney-client relationship with the firm begins only \
                 with a signed retainer.",
            )],
            vec![
                Run::plain(
                    "Every licence is scoped and quoted in conversation, and legal aid and \
                     nonprofit deployments should ask. ",
                ),
                Run::link("Ask us what yours would involve", "/contact"),
                Run::plain("."),
            ],
        ],
    }
}

/// `/services` — the published flat-fee schedule.
///
/// The routine end of the practice that is neither a dispute nor ongoing
/// counsel: the one-time consumer matters a person actually walks in with.
/// Every one carries its fee.
///
/// Publishing them is the decision this page embodies. A prospective client
/// who has been told all their life that a lawyer is unaffordable will not
/// book a consultation to find out; they will assume the answer and not
/// call. A number on the page answers that before the conversation, and it
/// binds the firm to charge the same person the same amount for the same
/// work, which is the part that makes it fair rather than merely
/// convenient.
///
/// Litigation and fractional general counsel are deliberately absent. Their
/// scope is not knowable in advance, so a published figure there would be
/// either a guess or a floor dressed as a price; both pages quote through
/// `/contact` and say so.
pub fn legal_services() -> PageContent {
    PageContent {
        head_title: format!("Legal Services — {}", views::brand::FIRM_BRAND.site_name),
        meta_description: "Flat-fee legal services from Neon Law: wills, trusts, name \
                           changes, formations, trademarks, and tenant defense, each on a \
                           fixed fee and reviewed by a licensed attorney."
            .to_string(),
        title: "Legal Services".to_string(),
        hero_mark: Some(webapp::components::PracticeMark::Gavel),
        tagline: "Once-Billed Legal Services".to_string(),
        // Two lines: the billing in the firm's colour, the noun under it in
        // text. Hyphenated, so "once" reads as how often rather than as when.
        hero_lines: vec![hero_line("Once-Billed", 1), hero_line("Legal Services", 0)],
        hero_lead: "The routine matters a person or a company walks in with, each scoped and \
                    quoted before any work begins."
            .to_string(),
        hero_cta: Some(HeroCta {
            href: "/contact".to_string(),
            label: "Contact us".to_string(),
        }),
        skin: webapp::marketing_page::PageSkin::Practice,
        bands: vec![
            legal_services_intro_band(),
            legal_services_fee_band(),
            legal_services_steps_band(),
            legal_services_cta_band(),
        ],
    }
}

/// The fee schedule itself: one card per matter, its price in the chip row.
///
/// A card rather than a table because the scope line has to travel with the
/// figure. A price list that showed only names and numbers would be read as
/// a quote for whatever the reader has in mind, and the boundary — one
/// property, one class, one hearing — is the difference between a fee the
/// firm can honour and one it will have to walk back.
fn legal_services_fee_band() -> Band {
    Band::Cards {
        anchor: "fees".to_string(),
        overline: "Flat fees".to_string(),
        heading: "The work we do at a flat fee".to_string(),
        description: Some(
            "Each of these is a fixed-fee matter: one scope, one price, agreed before we \
             start. Where a government body charges its own fee we pass it through at cost — \
             we do not mark it up, and we cannot control it. Email us for the fee on the \
             matter you need."
                .to_string(),
        ),
        items: FLAT_FEES
            .iter()
            .map(|entry| Card {
                title: entry.matter.to_string(),
                // No chip at all while the fee is unset. An empty chip
                // would render as a blank price tag, which reads worse
                // than no price tag.
                chips: entry.fee.map(str::to_string).into_iter().collect(),
                body: vec![vec![Run::plain(entry.scope)]],
                href: None,
                href_label: None,
            })
            .collect(),
    }
}

/// The closing call to action: contact the firm to get started.
fn legal_services_cta_band() -> Band {
    Band::Cta {
        heading: "Ready to get started?".to_string(),
        body: Some(
            "Tell us which matter you need and we will send you the flat fee for it before \
             any work begins. Email the firm to start."
                .to_string(),
        ),
        email: views::brand::firm_email().to_string(),
        email_subject: Some("Legal Services".to_string()),
    }
}

/// The line under the tagline: who the schedule is for, and the one
/// engagement it does not apply to.
fn legal_services_intro_band() -> Band {
    Band::Statement {
        heading: "Who this is for".to_string(),
        lead: String::new(),
        body: vec![vec![
            Run::plain(
                "These fees are for one-time matters. Business filings are already included \
                 in our ",
            ),
            Run::link("fractional GC", "/fractional-gc"),
            Run::plain(" projects, and a dispute is "),
            Run::link("litigation", "/litigation"),
            Run::plain(", which we quote per engagement."),
        ]],
    }
}

/// How the engagement runs — a short, fast, account-driven process, with a
/// licensed attorney's review before anything is filed.
fn legal_services_steps_band() -> Band {
    Band::Steps {
        anchor: "how".to_string(),
        overline: "How it works".to_string(),
        heading: "Our process is designed with speed in mind".to_string(),
        description: Some(
            "Create an account, answer some questions, upload your documentation, and we will \
             turn around and file what you need expeditiously."
                .to_string(),
        ),
        items: vec![
            Step {
                title: "Create an account".to_string(),
                body: vec![vec![Run::plain(
                    "Set up your account so everything about your matter lives in one place.",
                )]],
            },
            Step {
                title: "Answer some questions".to_string(),
                body: vec![vec![Run::plain(
                    "A short questionnaire, scoped to what your filing actually needs.",
                )]],
            },
            Step {
                title: "Upload your documentation".to_string(),
                body: vec![vec![Run::plain(
                    "Add the documents your matter calls for; we tell you which ones.",
                )]],
            },
            Step {
                title: "We file what you need, expeditiously".to_string(),
                body: vec![vec![
                    Run::plain("A licensed attorney reviews it, then we file it and send you "),
                    Run::plain("the confirmation when it comes back."),
                ]],
            },
        ],
    }
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
        assert!(
            text.contains("Co-Counsel a Pro Bono Case with Us"),
            "the only invitation is pro bono co-counsel: {text}"
        );
        assert!(
            text.contains(
                "serving clients as expeditiously, precisely, accurately, and in alignment with their interests"
            ),
            "the client-serving purpose is stated outright: {text}"
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
        assert!(
            !text.to_lowercase().contains("cto"),
            "no CTO offer reaches the page: {text}"
        );
        assert!(
            !text.to_lowercase().contains("ciso"),
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
            "becomes agpl-3.0-only four years",
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
            routes.iter().any(|href| href == "/contact"),
            "the licence *band* must route to `/contact`, where a licence is \
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
                Band::Cards { items, .. } => Some(items.as_slice()),
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
}
