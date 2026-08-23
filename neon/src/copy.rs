//! The Neon Law Foundation's public marketing copy.
//!
//! Moved here verbatim from the shared application crate: this is the
//! Foundation's own words about what it does, so it belongs to the binary that
//! publishes them rather than to the application both brands mount.

/// The Foundation's public marketing copy: its home page and the three
/// audience pages beneath it.
///
/// Borrowed from the retired `marketing/neon-law-foundation` static site
/// (ENG-139), which is the Foundation's own considered explanation of what it
/// does. Two things were deliberately *not* carried across:
///
/// * **The dated CLE announcement.** The static site named a specific session,
///   date, and partnering bar section on its home page. A named future event
///   is a claim that rots, and it named a third party; the education page
///   states the standing programme instead.
/// * **Unqualified accreditation.** The static site's own README recorded that
///   its CLE claims were "deliberately hedged" pending per-state
///   accreditation. That hedge is kept explicit here rather than inherited by
///   accident.
mod foundation_copy {
    use webapp::foundation_marketing::{Band, Card, Hero, HomeContent, PageContent, Run, Step};

    /// The single address every call to action opens. The Foundation takes
    /// intake by conversation, not by form.
    fn email() -> String {
        views::brand::foundation_email().to_string()
    }

    fn cta(heading: &str, body: Option<&str>) -> Band {
        Band::Cta {
            heading: heading.to_string(),
            body: body.map(str::to_string),
            email: email(),
            email_subject: None,
        }
    }

    /// The Foundation's home page: its identity, its argument, its programs,
    /// how a matter travels, and what it holds to.
    pub fn home() -> HomeContent {
        HomeContent {
            head_title: format!(
                "{} — access to justice, at speed",
                views::brand::FOUNDATION_BRAND.site_name
            ),
            meta_description: HOME_META_DESCRIPTION.to_string(),
            hero: home_hero(),
            bands: vec![
                mission_band(),
                programs_band(),
                how_it_works_band(),
                principles_band(),
                cta("Tell us about the matter.", None),
            ],
        }
    }

    /// The share card and search result for the home page.
    ///
    /// A meta description travels without its page, so it carries no bare
    /// "we teach CLEs": the accreditation qualification lives on `/education`
    /// and cannot follow a search snippet into a result list.
    const HOME_META_DESCRIPTION: &str =
        "The Neon Law Foundation is a 501(c)(3) nonprofit that pairs legal aid centers with \
         volunteer attorneys and AI technology to resolve legal matters quickly. We teach \
         continuing legal education, run trainings, and give every placed matter a free case \
         management workspace.";

    /// The opening screen: what the Foundation is, and the line it closes its
    /// argument on.
    fn home_hero() -> Hero {
        Hero {
            badge: "501(c)(3) nonprofit".to_string(),
            // The corporation, not the wordmark. `site_name` is "Neon Law",
            // which the header already wears an inch above this heading —
            // printing it again would leave a first-time visitor without the
            // organization's actual name, on the one page whose job is to say
            // what this 501(c)(3) is.
            title: views::brand::foundation_entity().to_string(),
            tagline: "Everyone in America should be able to exercise their legal rights."
                .to_string(),
            body: vec![
                vec![Run::plain(
                    "We are a 501(c)(3) nonprofit that pairs legal aid centers with volunteer \
                         attorneys and AI technology, so the matters sitting on a waitlist \
                         get taken up.",
                )],
                vec![
                    Run::plain(
                        "We teach the continuing legal education, we run the trainings, we \
                             make the match, and we give every matter a ",
                    ),
                    Run::strong("case management workspace at no cost"),
                    Run::plain("."),
                ],
            ],
            pullquote: "The technology takes the first draft. A lawyer does the deciding."
                .to_string(),
            email: email(),
        }
    }

    /// The mission band: why the Foundation exists at all.
    fn mission_band() -> Band {
        Band::Statement {
            heading: "Our mission".to_string(),
            lead: "The access-to-justice gap is not a shortage of law. It is a shortage \
                           of hours."
                .to_string(),
            body: vec![
                vec![
                    Run::plain(
                        "Legal aid centers turn away matters they have no capacity to staff, \
                         because the intake, the research, and the first draft take hours nobody \
                         has. Solo and small firm attorneys tell us they ",
                    ),
                    Run::strong("would take a pro bono matter"),
                    Run::plain(" if the work arrived scoped, organized, and ready to move."),
                ],
                vec![
                    Run::plain(
                        "We believe recent advances in technology can collapse the hours \
                                 without collapsing the judgment. Used carefully, and reviewed \
                                 adversarially, AI takes the ",
                    ),
                    Run::strong("weeks out of a matter and leaves the lawyering in"),
                    Run::plain("."),
                ],
            ],
        }
    }

    /// The three programs, each deep-linking to the page that expands it.
    fn programs_band() -> Band {
        Band::Cards {
            anchor: "what-we-do".to_string(),
            overline: "The programs".to_string(),
            heading: "What we do".to_string(),
            description: None,
            items: vec![
                Card {
                    title: "Education and CLEs".to_string(),
                    chips: vec![
                        "Continuing legal education".to_string(),
                        "Hands-on workshops".to_string(),
                        "AI-assisted drafting".to_string(),
                        "Adversarial review".to_string(),
                        "Clinic staff training".to_string(),
                    ],
                    body: vec![
                        vec![
                            Run::plain(
                                "Using AI in legal work is a skill, and it has to survive \
                                 professional scrutiny. We teach it directly: ",
                            ),
                            Run::strong(
                                "how to draft with a model, how to check it, and when to \
                                 throw its answer away",
                            ),
                            Run::plain("."),
                        ],
                        vec![Run::plain(
                            "Sessions are built for practitioners, not for demos, and are \
                                     free to our partners.",
                        )],
                    ],
                    href: Some(portal::dioxus_app::FOUNDATION_EDUCATION_PATH.to_string()),
                    href_label: Some("See the curriculum".to_string()),
                },
                Card {
                    title: "Pairing centers with attorneys".to_string(),
                    chips: vec![
                        "Intake triage".to_string(),
                        "Matter matching".to_string(),
                        "Solo and small firm volunteers".to_string(),
                        "Scoped engagements".to_string(),
                    ],
                    body: vec![
                        vec![
                            Run::plain(
                                "Legal aid centers have more matters than capacity. Solo \
                                         attorneys have capacity but no reliable way to find work \
                                         that fits. ",
                            ),
                            Run::strong("We close that gap."),
                        ],
                        vec![Run::plain(
                            "We take the matters a center cannot staff and match them to \
                                     volunteer attorneys by practice area, jurisdiction, and the \
                                     scope they can actually commit to.",
                        )],
                    ],
                    // No deep link: the audience page this card used to open is
                    // retired. The card's own prose is what the reader gets, and
                    // a center that wants to talk to us reaches the Foundation
                    // through the contact address in the footer.
                    href: None,
                    href_label: None,
                },
                Card {
                    title: "Free case management".to_string(),
                    chips: vec![
                        "No cost".to_string(),
                        "Deadlines and tasks".to_string(),
                        "Document assembly".to_string(),
                        "Visibility for the referring center".to_string(),
                    ],
                    body: vec![
                        vec![
                            Run::plain("Every matter we place comes with a workspace, and it is "),
                            Run::strong("free for the attorney and free for the center"),
                            Run::plain(
                                ". Deadlines, documents, and the record of what happened \
                                         all live in one place.",
                            ),
                        ],
                        vec![Run::plain(
                            "A solo taking a pro bono matter should not have to buy \
                                     software to do it.",
                        )],
                    ],
                    href: Some(portal::dioxus_app::FOUNDATION_ATTORNEYS_PATH.to_string()),
                    href_label: Some("For attorneys".to_string()),
                },
            ],
        }
    }

    /// How a matter travels, from the center's waitlist to a closed record.
    fn how_it_works_band() -> Band {
        Band::Steps {
            anchor: "how-it-works".to_string(),
            overline: "The pairing".to_string(),
            heading: "How it works".to_string(),
            description: Some(
                "From a matter a center cannot staff to a resolution, with a licensed \
                         attorney accountable at every step."
                    .to_string(),
            ),
            items: vec![
                Step {
                    title: "A center brings us a matter it cannot staff".to_string(),
                    body: vec![vec![Run::plain(
                        "Our legal aid partners send us the matters that would otherwise \
                                 sit on a waitlist. We work from the center's own intake, so \
                                 nobody has to re-tell their story to get help.",
                    )]],
                },
                Step {
                    title: "We match it to a volunteer attorney".to_string(),
                    body: vec![vec![
                        Run::plain("We match on practice area, jurisdiction, and the "),
                        Run::strong("scope the attorney can genuinely commit to"),
                        Run::plain(
                            ". A limited-scope engagement that gets finished beats a full \
                                     representation that stalls.",
                        ),
                    ]],
                },
                Step {
                    title: "The matter gets a workspace, at no cost".to_string(),
                    body: vec![vec![Run::plain(
                        "Deadlines, tasks, documents, and correspondence are set up \
                                 before the attorney's first call. The referring center keeps \
                                 visibility into status without having to chase anyone for an \
                                 update.",
                    )]],
                },
                Step {
                    title: "AI takes the first pass; the attorney decides".to_string(),
                    body: vec![
                        vec![Run::plain(
                            "Intake summaries, issue spotting, and first drafts are prepared \
                             before the attorney opens the matter. That is the part of the work \
                             that makes pro bono expensive in hours.",
                        )],
                        vec![
                            Run::strong(
                                "Nothing leaves the workspace without an attorney \
                                         reviewing it.",
                            ),
                            Run::plain(
                                " The model drafts. The lawyer is responsible, and the \
                                         lawyer signs.",
                            ),
                        ],
                    ],
                },
                Step {
                    title: "The matter closes and the center gets the record".to_string(),
                    body: vec![vec![Run::plain(
                        "When the matter resolves, the center receives a complete record \
                                 of what was done. What we learn from it goes back into the \
                                 trainings, so the next volunteer starts further along than the \
                                 last one did.",
                    )]],
                },
            ],
        }
    }

    /// The three commitments the Foundation measures itself against.
    fn principles_band() -> Band {
        Band::Cards {
            anchor: "principles".to_string(),
            overline: "How we run".to_string(),
            heading: "What we hold to".to_string(),
            description: None,
            items: vec![
                Card {
                    title: "Speed is the point".to_string(),
                    chips: Vec::new(),
                    body: vec![vec![
                        Run::plain(
                            "A right you cannot exercise in time is not much of a right. We \
                             measure ourselves on ",
                        ),
                        Run::strong("how long a matter actually takes"),
                        Run::plain("."),
                    ]],
                    href: None,
                    href_label: None,
                },
                Card {
                    title: "A human is accountable".to_string(),
                    chips: Vec::new(),
                    body: vec![vec![
                        Run::plain("AI drafts, summarizes, and organizes. A "),
                        Run::strong("licensed attorney reviews and decides"),
                        Run::plain(
                            ", every time. We will not ship a robot lawyer, and we will \
                                     not pretend one exists.",
                        ),
                    ]],
                    href: None,
                    href_label: None,
                },
                Card {
                    // "Free or discounted", stated plainly and not embellished.
                    // A Statement of Legal Aid certifies a client for
                    // reduced-cost help, so some placed matters carry a fee
                    // and an unqualified "free" would be wrong for them. Court
                    // filing fees are not ours to waive either, and a client
                    // who read otherwise meets the clerk's window believing it.
                    title: "Free or discounted".to_string(),
                    chips: Vec::new(),
                    body: vec![vec![Run::plain(
                        "No cost to the legal aid organization, and no cost to the \
                                 volunteer attorney. For the client, legal fees are free or \
                                 reduced, set by the organization that certified them. Court \
                                 filing fees and similar costs are not ours to waive; we tell \
                                 you what they are before they land.",
                    )]],
                    href: None,
                    href_label: None,
                },
            ],
        }
    }

    /// `/education` — the CLE and training curriculum.
    pub fn education() -> PageContent {
        PageContent {
            skin: webapp::foundation_marketing::PageSkin::Marketing,
            head_title: format!("Education and CLE — {}", views::brand::FOUNDATION_BRAND.site_name),
            meta_description:
                "The Neon Law Foundation teaches continuing legal education and hands-on training \
                 on using AI in legal practice with judgment, collaboration, and adversarial \
                 review. Free to our legal aid partners and volunteer attorneys. Ask us which \
                 credits a session carries in your jurisdiction."
                    .to_string(),
            title: "Education and CLE".to_string(),
            hero_mark: None,
            tagline: "How to draft with a model, how to check it, and when to throw its answer \
                      away."
                .to_string(),
            bands: vec![
                Band::Statement {
                    heading: "Why we teach".to_string(),
                    lead: "Using AI in legal work is a skill, and it has to survive professional \
                           scrutiny."
                        .to_string(),
                    body: vec![
                        vec![Run::plain(
                            "The competence and confidentiality duties did not change when the \
                             tools did. What changed is that an attorney can now produce a \
                             plausible draft in seconds without any of the reasoning that makes \
                             it defensible.",
                        )],
                        vec![
                            Run::plain("Our sessions are built for practitioners, not for demos. "),
                            Run::strong(
                                "Every exercise runs on the kind of matter a legal aid center \
                                 actually sees",
                            ),
                            Run::plain(", and every output goes through review before anyone relies on it."),
                        ],
                    ],
                },
                education_curriculum_band(),
                education_accreditation_band(),
                cta(
                    "Bring us to your clinic.",
                    Some(
                        "Tell us who you train and what they are stuck on, and we will put a \
                         session together.",
                    ),
                ),
            ],
        }
    }

    /// What the Foundation actually teaches.
    fn education_curriculum_band() -> Band {
        Band::Cards {
                    anchor: "curriculum".to_string(),
                    overline: "The curriculum".to_string(),
                    heading: "What we cover".to_string(),
                    description: None,
                    items: vec![
                        Card {
                            title: "Drafting with a model".to_string(),
                            chips: vec![
                                "AI-assisted drafting".to_string(),
                                "Prompting for legal work".to_string(),
                            ],
                            body: vec![vec![Run::plain(
                                "Where a model earns its place in a drafting workflow, and where \
                                 it costs more time than it saves. We work from real intake and \
                                 real pleadings rather than toy prompts.",
                            )]],
                            href: None,
                            href_label: None,
                        },
                        Card {
                            title: "Adversarial review".to_string(),
                            chips: vec![
                                "Adversarial review".to_string(),
                                "Confidentiality and competence".to_string(),
                            ],
                            body: vec![vec![
                                Run::plain("The habit that makes the rest safe: assume the first answer is wrong and go looking for how. "),
                                Run::strong("An attorney who cannot say why a draft is right should not file it."),
                            ]],
                            href: None,
                            href_label: None,
                        },
                        Card {
                            title: "Training the trainers".to_string(),
                            chips: vec![
                                "Clinic staff training".to_string(),
                                "Train the trainer".to_string(),
                            ],
                            body: vec![vec![Run::plain(
                                "Standing sessions for legal aid staff, volunteer attorneys, and \
                                 law students, so a center can carry the practice without us in \
                                 the room.",
                            )]],
                            href: None,
                            href_label: None,
                        },
                    ],
        }
    }

    /// The hedge, stated on the page rather than left to be discovered.
    ///
    /// Continuing legal education is accredited state by state. The retired
    /// static site kept this qualification in its README; a reader deciding
    /// whether a session counts for them needs it on the page.
    fn education_accreditation_band() -> Band {
        Band::Statement {
            heading: "Accreditation".to_string(),
            lead: "On accreditation, we would rather under-promise.".to_string(),
            body: vec![vec![
                Run::plain(
                    "Continuing legal education is accredited state by state, and a \
                             session that carries credit in one jurisdiction may not in another. ",
                ),
                Run::strong(
                    "Ask us which credits a given session carries in your jurisdiction \
                             before you rely on it",
                ),
                Run::plain(", and we will tell you plainly — including when the answer is none."),
            ]],
        }
    }

    /// `/attorneys` — the pitch to volunteer attorneys.
    pub fn attorneys() -> PageContent {
        PageContent {
            skin: webapp::foundation_marketing::PageSkin::Marketing,
            head_title: format!(
                "For volunteer attorneys — {}",
                views::brand::FOUNDATION_BRAND.site_name
            ),
            meta_description:
                "Take a pro bono matter that arrives scoped, organized, and ready to move. The \
                 Neon Law Foundation provides free case management, AI-assisted first drafts, and \
                 continuing legal education to volunteer attorneys."
                    .to_string(),
            title: "For volunteer attorneys".to_string(),
            hero_mark: None,
            tagline: "Pro bono work that arrives scoped, organized, and ready to move.".to_string(),
            bands: vec![
                Band::Statement {
                    heading: "What we are asking".to_string(),
                    lead: "The barrier to pro bono is the hours before the lawyering starts."
                        .to_string(),
                    body: vec![vec![
                        Run::plain(
                            "Intake, records, and the first draft. We do that part and hand you \
                             a matter with ",
                        ),
                        Run::strong("a scope you agreed to before you said yes"),
                        Run::plain("."),
                    ]],
                },
                Band::Cards {
                    anchor: "what-you-get".to_string(),
                    overline: "What you get".to_string(),
                    heading: "What comes with the matter".to_string(),
                    description: None,
                    items: vec![
                        Card {
                            title: "A workspace, at no cost".to_string(),
                            chips: vec!["No cost".to_string(), "Deadlines and tasks".to_string()],
                            body: vec![vec![Run::plain(
                                "Deadlines, documents, and correspondence in one place, set up \
                                 before your first call.",
                            )]],
                            href: None,
                            href_label: None,
                        },
                        Card {
                            title: "A first pass already done".to_string(),
                            chips: vec!["Document assembly".to_string()],
                            body: vec![vec![
                                Run::plain("Intake summaries, issue spotting, and first drafts. "),
                                Run::strong("You review and decide"),
                                Run::plain(" — nothing goes out without that."),
                            ]],
                            href: None,
                            href_label: None,
                        },
                        Card {
                            // Not "training that counts": the title asserted the
                            // credit the body then hedges, on the page read by
                            // the attorneys most likely to rely on it. The
                            // Foundation holds no provider accreditation, so
                            // every credit-adjacent surface carries the same
                            // qualification, including the half that says none.
                            title: "Training, free to volunteers".to_string(),
                            chips: vec!["Continuing legal education".to_string()],
                            body: vec![vec![Run::plain(
                                "Our CLE and AI-in-practice sessions are free to volunteers. Ask \
                                 us which credits a session carries in your jurisdiction before \
                                 you rely on it. We will tell you, including when the answer is \
                                 none.",
                            )]],
                            href: Some(portal::dioxus_app::FOUNDATION_EDUCATION_PATH.to_string()),
                            href_label: Some("See the curriculum".to_string()),
                        },
                    ],
                },
                attorneys_responsibility_band(),
                cta(
                    "Tell us what you practice.",
                    Some(
                        "Practice areas, jurisdictions, and the scope you can commit to. We will \
                         match you when something fits.",
                    ),
                ),
            ],
        }
    }

    /// Who is responsible for what, in both directions.
    ///
    /// The first paragraph is the Foundation's own separation: it is not
    /// co-counsel and does not supervise the volunteer. Stated alone that is
    /// dangerously incomplete — a nonlawyer organization spotting issues and
    /// preparing drafts that nobody supervises describes the unauthorized
    /// practice of law rather than disclaiming it. The second paragraph
    /// supplies the direction that makes the arrangement lawful: the attorney
    /// supervises the Foundation's work product, runs their own conflicts
    /// check, and holds the confidentiality duty the Foundation works under.
    /// Neither paragraph may ship without the other.
    fn attorneys_responsibility_band() -> Band {
        Band::Statement {
            heading: "Where the responsibility sits".to_string(),
            lead: "You are the lawyer on the matter. That does not move.".to_string(),
            body: vec![
                vec![
                    Run::plain(
                        "The Foundation is not co-counsel and does not supervise your \
                         representation. We assemble, organize, and draft; ",
                    ),
                    Run::strong("the professional judgment and the signature are yours"),
                    Run::plain("."),
                ],
                vec![
                    Run::strong("You supervise the work we prepare"),
                    Run::plain(
                        ". What we hand you is a draft until you say otherwise. You run your own \
                         conflicts check before accepting a matter. What we hold about your \
                         client, we hold under your duty of confidentiality.",
                    ),
                ],
            ],
        }
    }
}

pub use foundation_copy::*;

/// The Foundation's public copy carries legal weight, so the claims a
/// legal-council review settled are pinned here rather than left to a future
/// edit's good intentions.
///
/// The Foundation holds no CLE provider accreditation in any jurisdiction, and
/// the three states its attorneys are admitted in do not agree on the cure:
/// Washington lets an attendee self-submit an unaccredited activity, Nevada
/// requires a non-accredited provider to file 30 days ahead, and California
/// gates credit on State Bar provider approval with no attendee-side fix. A
/// hedge that lives on one page therefore is not enough — every surface that
/// mentions credit carries the qualification with it.
#[cfg(test)]
mod foundation_copy_tests {
    use super::foundation_copy;
    use webapp::foundation_marketing::{Band, Paragraph};

    /// Every word of prose a band renders, flattened. Titles, leads, chips,
    /// and card bodies all count: a reader does not distinguish the struct
    /// field a claim arrived in.
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
            Band::Cards { heading, items, .. } => {
                let cards = items
                    .iter()
                    .map(|c| format!("{} {} {}", c.title, c.chips.join(" "), paragraphs(&c.body)))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{heading} {cards}")
            }
            Band::Steps { heading, items, .. } => {
                let steps = items
                    .iter()
                    .map(|s| format!("{} {}", s.title, paragraphs(&s.body)))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{heading} {steps}")
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

    fn page_text(bands: &[Band]) -> String {
        bands.iter().map(band_text).collect::<Vec<_>>().join(" ")
    }

    /// The disclaimer runs both directions.
    ///
    /// Saying only that the Foundation does not supervise the attorney leaves
    /// a nonlawyer organization issue-spotting and drafting with nobody
    /// supervising it at all — which describes the unauthorized practice of
    /// law rather than disclaiming it. The page must also say the attorney
    /// supervises the Foundation's work, runs their own conflicts check, and
    /// that client information sits under the attorney's confidentiality duty.
    #[test]
    fn attorneys_page_puts_the_foundations_work_under_the_attorneys_supervision() {
        let text = page_text(&foundation_copy::attorneys().bands);
        assert!(
            text.contains("does not supervise your representation"),
            "the Foundation's own non-supervision survives: {text}"
        );
        assert!(
            text.contains("You supervise the work we prepare"),
            "the missing direction — the attorney supervises our work: {text}"
        );
        assert!(
            text.contains("conflicts check"),
            "the volunteer runs their own conflicts check: {text}"
        );
        assert!(
            text.contains("duty of confidentiality"),
            "client information sits under the attorney's duty: {text}"
        );
    }

    /// No surface promises CLE credit the Foundation cannot deliver.
    ///
    /// "Training that counts" asserted in its title the credit its body then
    /// hedged. Wherever the copy raises credit, the qualification travels with
    /// it — including the half that admits the answer may be none.
    #[test]
    fn every_credit_claim_carries_its_qualification() {
        let attorneys = page_text(&foundation_copy::attorneys().bands);
        assert!(
            !attorneys.contains("Training that counts"),
            "the title asserted the credit the body hedges: {attorneys}"
        );
        assert!(
            attorneys.contains("including when the answer is none"),
            "the honest half of the hedge reaches the attorneys page: {attorneys}"
        );

        let education = page_text(&foundation_copy::education().bands);
        assert!(
            education.contains("accredited state by state")
                && education.contains("including when the answer is none"),
            "the education hedge stands: {education}"
        );
    }

    /// A meta description travels without its page.
    ///
    /// It is the search snippet and the share card, so it cannot lean on a
    /// qualification that lives three screens down a page the reader has not
    /// opened. None of them may claim credit outright.
    #[test]
    fn meta_descriptions_make_no_bare_cle_claim() {
        let home = foundation_copy::home();
        let education = foundation_copy::education();
        let attorneys = foundation_copy::attorneys();
        for (page, description) in [
            ("home", home.meta_description.as_str()),
            ("education", education.meta_description.as_str()),
            ("attorneys", attorneys.meta_description.as_str()),
        ] {
            assert!(
                !description.contains("CLEs") && !description.contains("CLE training"),
                "the {page} meta description claims credit without its hedge: {description}"
            );
        }
        assert!(
            education
                .meta_description
                .contains("Ask us which credits a session carries"),
            "the education snippet carries the ask with it: {}",
            education.meta_description
        );
    }

    /// The cost promise matches what a placed matter actually costs.
    ///
    /// Two ways it could overrun. A Statement of Legal Aid certifies a client
    /// for *reduced-cost* help, so some placed matters carry a fee and a flat
    /// "free" would be wrong for those clients. And court filing fees are not
    /// the Foundation's to waive at all — a client who read otherwise meets
    /// the clerk's window believing it. The copy states both plainly and
    /// boasts about neither.
    #[test]
    fn the_cost_promise_admits_a_fee_and_the_costs_it_cannot_waive() {
        let home = foundation_copy::home();
        let text = page_text(&home.bands);
        for overclaim in [
            "No cost to the client",
            "and we mean free",
            "No legal fees for the client",
        ] {
            assert!(
                !text.contains(overclaim),
                "{overclaim:?} promises more than a SOLA-certified matter delivers: {text}"
            );
        }
        assert!(
            text.contains("Free or discounted"),
            "the heading says which it is: {text}"
        );
        assert!(
            text.contains("legal fees are free or reduced"),
            "and the body admits a fee is possible: {text}"
        );
        assert!(
            text.contains("Court filing fees and similar costs are not ours to waive"),
            "and names the costs we cannot waive at all: {text}"
        );
    }
}
