//! The site footer, as a Dioxus component (issue #641, Phase 2).
//!
//! Two bands. The contact band reaches the firm: the email CTA, the published
//! voice line, and every office it keeps. Below it the legal strip carries the
//! load-bearing, brand-driven lines every public page owes — the copyright that
//! names the legal person behind the site, which attorney holds which bar
//! licence, and the attorney-advertising disclaimer.
//!
//! It is prop-driven like [`crate::components::PricingSection`]: the process
//! brand (`views::brand`) is mapped onto [`SiteFooterLegal`]'s props per request
//! on the server, so the wasm client never links the view layer and a
//! white-label deploy emits its own identity. `crate::public_chrome::PublicFooter`
//! owns that mapping so no page restates it.
//!
//! The copy is legal-council-reviewed and must not drift. Bar disclosure is per
//! attorney and never firm-level: every published number links to the bar's own
//! record, so a visitor verifies a licence against the licensing jurisdiction
//! rather than trusting a summary line the page wrote about itself.

use dioxus::prelude::*;

use crate::components::{ExternalLink, GitHubStars, Icon, IconName};

/// One published office — the state it sits in and its street address.
/// Mirrors `views::brand::FirmOffice`.
#[derive(Clone, PartialEq, Eq)]
pub struct FooterOffice {
    pub state: String,
    pub address: String,
    /// A qualification rendered under the address — e.g. an admission that has
    /// not issued yet. `None` publishes the address unqualified. See
    /// `views::brand::FirmOffice::note` for why this rides the address.
    pub note: Option<String>,
}

/// Outlines of the states the firm publishes an office in, each derived from
/// the state's real border geometry rather than drawn by hand.
///
/// The source is Wikimedia Commons' `Blank US Map (states only).svg`, released
/// under CC0, whose per-state paths carry the actual border coordinates in one
/// shared projection. Each state's path was lifted from that file, simplified
/// with Douglas–Peucker to a few dozen vertices, and fitted — **at its true
/// aspect ratio**, centred — into this 100×100 box. Aspect ratio is the reason
/// the earlier hand-traced set read wrong: a state stretched to fill a square
/// stops being that state's silhouette, whatever its corners do.
///
/// Fitting to the larger dimension means a wide state (Washington) leaves space
/// above and below and a tall one (California, Nevada) leaves it to the sides,
/// so all four render at a consistent scale beside one another.
///
/// **Deliberately not the state flags.** New York's flag is the state coat of
/// arms and Washington's is the state seal; both states restrict use of those
/// devices in advertising, and a law-firm footer is advertising copy. An
/// outline carries the same "this is where we are" meaning and claims nothing
/// about who endorses the firm. It is also legible at one colour and one low
/// opacity, which four multi-colour flags behind body text would not be.
///
/// A state with no entry renders no watermark, so a white-label deploy that
/// publishes an office somewhere else simply gets the plain treatment.
const STATE_OUTLINES: &[(&str, &str)] = &[
    (
        "California",
        "M53.6 95 52.4 87.7 48.9 83.2 46.7 82.4 46.9 80.3 43.4 79.1 40 74.9 34.4 73.1 33.4 72.1 \
         34.5 67.1 31.3 61.3 29.8 56.3 28.7 55.3 28.9 52.5 30.6 50.9 30.4 49.7 29 49.3 27.5 46.7 \
         28 41.3 29 41.3 28.6 43.2 30.1 44.5 29.3 40.2 30.4 39.6 29.7 38.4 29.1 38.6 28.1 41 26 \
         38.6 25.8 34.9 23.3 29.8 24.6 21.7 22.4 17.4 22.5 15.3 25.9 11.4 27.4 7.3 27.8 3 53.7 \
         10.3 47.3 35.1 75.4 77.6 75 78.4 77.6 84.5 75.2 85.7 74.3 87.1 73.6 89.8 72.1 91.2 71.6 \
         93.9 73.2 95.2 72.9 96.2 70.8 97Z",
    ),
    (
        "Nevada",
        "M68.9 74.4 67.1 84 65.7 85.6 64.7 85.6 64 84.1 62 83.3 60.1 83.6 59.8 93.8 58.7 97 20.2 \
         39.5 19.6 37.6 28.6 3 80.4 14.7Z",
    ),
    // Two subpaths: the mainland, and Long Island east of the city.
    (
        "New York",
        "M73.5 84.3 75.7 84.8 83 81.7 80 82 86.4 79.6 97 71.1 92.5 73.4 91.3 72.1 90.5 75.5 89.1 \
         75.5 92.5 70.3 87.7 75 77 78.7 73.6 82.8ZM65.8 16.5 66.4 21.8 68 23.4 67.6 29.8 69.8 \
         34.6 69.2 35.8 69.9 37.4 70.5 36.3 71.9 37.3 74.4 49.5 74.1 60.2 76.6 72.5 77.8 73.5 \
         75.3 75.9 76.6 77.5 73.5 82.7 73.6 78 60.8 74 53.6 66.4 3.6 76.3 3 72 11.9 62.4 8.2 \
         57.6 8 55.3 13.4 52.5 19.3 51.4 25.3 52.4 33 49.9 36.4 45.8 38.6 45.2 38.9 42.9 37 40.5 \
         38.4 38.3 35.7 37.3 35.6 35.1 47.3 20.1 65.2 15.2Z",
    ),
    (
        "Washington",
        "M86.8 77.6 86.5 84.2 62.2 78.4 34.4 78.4 30.4 74.9 20.7 75.8 15.4 72.5 16 64.7 3 57 5 51 \
         4.8 56.2 8.4 51.4 5.5 49.8 5.6 46.4 9.6 46.7 5.3 45.5 5.5 22.9 7.4 19.5 13.7 25.6 24.1 \
         29.1 24.3 31.5 28.7 31.2 27.7 35 19.4 42 23 42.6 20.2 41.8 29.4 34.5 29.5 37.7 25.7 39.8 \
         25.6 46.2 24.7 43.6 22.9 46.6 23.3 43.3 19.2 47 22.9 48.6 28.8 45.3 29.1 39.4 33 35.2 \
         31.1 26 31.6 28.5 28.8 29.2 31.5 35.3 27.9 28.9 30.6 23.2 32.4 25.8 33.4 23.9 30.9 20.2 \
         32.3 15.8 97 32.6Z",
    ),
];

/// The outline for a published state, or `None` for one this component does not
/// carry a tracing of.
fn state_outline(state: &str) -> Option<&'static str> {
    STATE_OUTLINES
        .iter()
        .find(|(name, _)| *name == state)
        .map(|(_, outline)| *outline)
}

/// One page the footer links. Mirrors `views::brand::NavLink`, narrowed to what
/// a flat footer row renders — the footer has no dropdowns.
#[derive(Clone, PartialEq, Eq)]
pub struct FooterNavLink {
    pub label: String,
    pub href: String,
}

/// One bar license an attorney holds. Mirrors `views::brand::BarLicense`.
#[derive(Clone, PartialEq, Eq)]
pub struct FooterBarLicense {
    pub jurisdiction: String,
    pub number: String,
    pub license_url: String,
}

/// One licensed attorney and the set of bar licenses they hold. Mirrors
/// `views::brand::FirmAttorney`.
#[derive(Clone, PartialEq, Eq)]
pub struct FooterAttorney {
    pub name: String,
    pub licenses: Vec<FooterBarLicense>,
}

/// The lines a published address is set over.
///
/// An address is published as one string, here and in a white-label manifest
/// (`brand.firm_offices[].address`), because that is how a firm writes its own
/// address. The footer sets it the way an envelope carries it — street, then
/// unit, then city — so the suite gets its own line and the city a reader is
/// scanning for starts one, instead of the whole address running together and
/// breaking wherever the narrow column happens to end.
///
/// Every comma starts a new line, except the one between the city and its
/// state: `Walnut Creek, CA 94596` is one line, because a city split from its
/// ZIP stops reading as a place. So the last two of three or more
/// comma-separated parts are the final line, and each part before them is a
/// line of its own. An address written with no comma at all is published as the
/// one line it was written as, rather than broken at a guess.
fn address_lines(address: &str) -> Vec<&str> {
    let mut commas = address.rmatch_indices(',').map(|(index, _)| index);
    // The last comma separates the city from `ST ZIP` and is not a break; the
    // one before it ends the last line above the city.
    commas.next();
    let (above, city) = match commas.next() {
        Some(index) => (&address[..index], Some(address[index + 1..].trim_start())),
        None => (address, None),
    };
    let mut lines: Vec<&str> = above
        .split(',')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.extend(city);
    lines
}

/// The footer's email tile opens a message that is already filed. A mail
/// client shows the subject before the body, so naming what the sender wants —
/// legal services — is what lets the firm route the message on sight rather
/// than reading an untitled note to find out. `%20` rather than `+`: only the
/// percent form decodes to a space in every client's `mailto:` handler.
///
/// `pub(crate)` rather than private: [`crate::litigation_page`] reuses it for
/// the same channel link outside the footer, and a second copy of the subject
/// line would drift from this one silently.
pub(crate) fn mailto_href(email: &str) -> String {
    format!("mailto:{email}?subject=Legal%20services.")
}

/// `tel:` dials digits, not the human spacing a number is written with.
/// `pub(crate)` for the same reason as [`mailto_href`].
pub(crate) fn tel_href(phone: &str) -> String {
    format!(
        "tel:{}",
        phone
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect::<String>()
    )
}

/// The footer legal strip. Every field is resolved from the deploy's brand and
/// handed in per request, so the component itself is pure presentation.
///
/// - `copyright_holder`: the legal person that owns the site and the words on
///   it — `Shook Law PLLC` on the firm's deploy. It heads the legal strip, and
///   it is the only line naming the entity behind the site. Deliberately a
///   separate dial from the wordmark the page trades under, even where a
///   deploy sets both to the same words: a copyright notice has to name an
///   entity that can hold one, and a white-label bundle renames the wordmark
///   without renaming the copyright holder.
/// - `disclaimer`: the attorney-advertising disclaimer copy.
/// - `copyright_year`: the year on the copyright line, resolved by the
///   server per request so the reusable component never carries a stale year.
/// - `contact_email` / `phone` / `offices`: the firm's published contact
///   channels. Each is independently optional — an empty value renders
///   nothing, so a deploy that publishes no voice line or no walk-in office
///   simply omits it rather than rendering an empty label. With all three
///   empty the contact band disappears and only the legal strip remains.
/// - `attorneys`: the licensed attorneys and the bar licences each holds, one
///   line per attorney, every number linked to that bar's own record. Empty
///   renders no licence list.
/// - `logo_href` / `brand_name`: the mark the footer opens on and the wordmark
///   beside it. `crate::public_chrome::PublicFooter` feeds both from the firm
///   brand on every page of the site, so the bottom of a page names one
///   organization whichever header a white-label bundle mounts above it.
/// - `home_href`: where that mark links. A reader who has scrolled to the
///   bottom of a long page clicks the logo to get back to the top of the site,
///   and the header's mark is off screen by then — so the footer's has to be
///   the same door rather than an inert picture. It is the *firm's* home under
///   any header, matching the mark and wordmark beside it.
///   Empty renders the mark unlinked, which is what a gallery driving the
///   component with no destination should get rather than an `<a href="">`
///   that reloads whatever page it is sitting on.
/// - `nav`: the public routes the header does not carry, rendered as one list
///   the stylesheet lays out in two columns of four on a wide viewport and one
///   column of eight on a narrow one. Empty renders no row.
///
/// The whole footer is width-constrained to the same 72rem column the
/// [`crate::components::SiteHeader`] nav uses, so its content lines up with
/// the navbar above it instead of running to the viewport edge. The rule and
/// background stay full-bleed, matching the header's `border-bottom`.
#[component]
pub fn SiteFooterLegal(
    copyright_holder: String,
    disclaimer: String,
    copyright_year: i32,
    #[props(default)] logo_href: String,
    #[props(default)] contact_email: String,
    #[props(default)] phone: String,
    #[props(default)] offices: Vec<FooterOffice>,
    #[props(default)] attorneys: Vec<FooterAttorney>,
    #[props(default)] brand_name: String,
    #[props(default)] home_href: String,
    #[props(default)] nav: Vec<FooterNavLink>,
    /// The registered word mark the site trades under, spelled the way the
    /// register spells it, and the registration that proves it. Renders one
    /// notice under the copyright line — the site's two ownership facts read
    /// together — naming the mark, the registrant, and the number, with the
    /// number linked to the register's own record so a reader verifies the
    /// claim there rather than trusting the site's line about itself. That is
    /// the same rule the bar-licence rows below follow.
    ///
    /// The registrant the notice names is `copyright_holder`, because on this
    /// deploy they are the same legal person and a fourth prop that could
    /// disagree with the third is a way to publish the wrong owner of a live
    /// registration. `views::brand` resolves both from the firm brand.
    ///
    /// `trademark` or `trademark_registration` empty renders no line at all,
    /// which is what a deploy holding no registration should publish; with
    /// `trademark_record_url` empty the notice renders unlinked.
    #[props(default)]
    trademark: String,
    #[props(default)] trademark_registration: String,
    #[props(default)] trademark_record_url: String,
    /// The public repository the platform is developed in — how it is named
    /// (`owner/name`), where it lives, and how many people have starred it.
    /// Closes the strip: no box, no attribution prose, just the repository,
    /// its star count, and the running release beside it on the same line.
    /// Both strings empty renders no line.
    ///
    /// `source_stars` is independently optional, and `None` is the ordinary
    /// case rather than a failure — see
    /// [`crate::source_repository`]. It renders the link with no count.
    #[props(default)]
    source_repo: String,
    #[props(default)] source_href: String,
    #[props(default)] source_stars: Option<u64>,
    /// The published release this deployment is running, and the page that
    /// describes the platform. Set right beside the repository link rather
    /// than on a line of its own, so the two halves of the same fact read
    /// together in one glance: this is the software, and this is the build of
    /// it serving the page.
    ///
    /// A push is visible end to end — the moment a new image is live, the
    /// footer's number changes. Both strings empty renders no line, which is
    /// the ordinary local case: `NAVIGATOR_RELEASE_TAG` is unset under
    /// `cargo run`, and a footer reading "#" is worse than no version at all.
    #[props(default)]
    navigator_version: String,
    #[props(default)] navigator_href: String,
) -> Element {
    let has_contact = !contact_email.is_empty() || !phone.is_empty() || !offices.is_empty();
    let has_masthead = !logo_href.is_empty() || !brand_name.is_empty() || !nav.is_empty();
    // The mark's contents, built once: the element around them is an anchor or
    // a plain box depending on whether the deploy published a home to link to,
    // and writing the image and wordmark out under each branch is how the two
    // drift apart.
    let mark = rsx! {
        if !logo_href.is_empty() {
            img { class: "site-footer__logo", src: "{logo_href}", alt: "" }
        }
        if !brand_name.is_empty() {
            strong { class: "site-footer__wordmark", "{brand_name}" }
        }
    };
    rsx! {
        footer { class: "site-footer", role: "contentinfo",
            div { class: "site-footer__inner",
                // The mark, the name it belongs to, and the link row, in one
                // element so they share one grid — and so that grid can be the
                // contact band's own, which is what lines the first link column
                // up with the first address tile below it. Two separate
                // children of `__inner` could not: the band insets its tiles by
                // its own padding, so a track measured outside the band never
                // lands where a track measured inside it does.
                if has_masthead {
                    div { class: "site-footer__masthead",
                        // The logo alone left the footer opening on an
                        // unlabelled glyph; the wordmark is the same one the
                        // header carries, so the bottom of the page says whose
                        // page it is. The image stays decorative (`alt=""`)
                        // because the text beside it is the label.
                        if !logo_href.is_empty() || !brand_name.is_empty() {
                            // Linked when there is a home to link to, and the
                            // anchor carries the accessible name for the same
                            // reason the header's does: the image is decorative
                            // and the wordmark beside it is the label, so a
                            // screen reader announcing both would say the brand
                            // twice.
                            if home_href.is_empty() {
                                div { class: "site-footer__brand", {mark} }
                            } else {
                                a {
                                    class: "site-footer__brand",
                                    href: "{home_href}",
                                    "aria-label": "{brand_name} home",
                                    {mark}
                                }
                            }
                        }
                        // The pages the header does not carry, as one list of
                        // the firm's own routes, so a reader who scrolled to the
                        // bottom looking for the Blog, Contact, or the platform
                        // page finds them before the contact band's detail.
                        //
                        // A list, because that is what it is: a set of sibling
                        // destinations a screen reader should announce with a
                        // count. One `<ul>` in the markup at every width — the
                        // two columns a wide viewport shows are the stylesheet
                        // flowing this single list down five rows and across, so
                        // the reading order and the announced count are the same
                        // everywhere.
                        if !nav.is_empty() {
                            nav { class: "site-footer__nav", "aria-label": "More pages",
                                ul { class: "site-footer__nav-list",
                                    for link in nav.iter() {
                                        li { class: "site-footer__nav-item", key: "{link.href}",
                                            a {
                                                class: "site-footer__nav-link",
                                                href: "{link.href}",
                                                "{link.label}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if has_contact {
                    div { class: "site-footer__contact",
                        // The email and phone channels are tiles in the same
                        // grid as the offices, and wear the same card the
                        // practice boxes on `/` do — one ground, one border,
                        // and the same swell-and-lift under a pointer. What
                        // marks them as the two actionable channels is that
                        // motion, not a fill: a brand-filled pill read as a
                        // pair of buttons shouting over the addresses beside
                        // them, and the footer is not where the page shouts.
                        ul { class: "site-footer__offices",
                            if !contact_email.is_empty() {
                                li { class: "site-footer__office site-footer__office--channel",
                                    a {
                                        class: "site-footer__channel-link",
                                        href: mailto_href(&contact_email),
                                        Icon { name: IconName::EnvelopeFill }
                                        span { "{contact_email}" }
                                    }
                                }
                            }
                            if !phone.is_empty() {
                                li { class: "site-footer__office site-footer__office--channel",
                                    a {
                                        class: "site-footer__channel-link",
                                        href: tel_href(&phone),
                                        Icon { name: IconName::TelephoneFill }
                                        span { "{phone}" }
                                    }
                                }
                            }
                            for office in offices.iter() {
                                li { class: "site-footer__office", key: "{office.state}",
                                        // The state behind its own address, as
                                        // a watermark. Decorative and
                                        // `aria-hidden`: the label above it
                                        // already names the state, so a screen
                                        // reader that announced this too would
                                        // hear the office twice.
                                        if let Some(outline) = state_outline(&office.state) {
                                            svg {
                                                class: "site-footer__office-map",
                                                "viewBox": "0 0 100 100",
                                                "aria-hidden": "true",
                                                "focusable": "false",
                                                fill: "currentColor",
                                                path { d: outline }
                                            }
                                        }
                                        span { class: "site-footer__office-label", "{office.state}" }
                                        // `<address>` is the semantic element
                                        // for the contact details of its
                                        // nearest ancestor section — here, the
                                        // firm that owns the page. Set line by
                                        // line — street, unit, city — the way
                                        // the envelope would carry it.
                                        address { class: "site-footer__office-address",
                                            for line in address_lines(&office.address) {
                                                span {
                                                    class: "site-footer__office-line",
                                                    key: "{line}",
                                                    "{line}"
                                                }
                                            }
                                        }
                                        // The qualification sits inside the
                                        // same `<li>` as the address it
                                        // qualifies, so no reader can pair it
                                        // with the wrong office.
                                        if let Some(note) = office.note.as_ref() {
                                            span { class: "site-footer__office-note", "{note}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                div { class: "site-footer__legal",
                    div { class: "site-footer__legal-practice",
                        p { class: "site-footer__copyright",
                        // The site and the words on it belong to the firm's
                        // legal person, which is what a copyright notice names
                        // — the wordmark cannot hold one. This heads the legal
                        // strip rather than trailing it, because it is the only
                        // line naming the entity behind the site. One name: the
                        // wordmark it trades under is named on the line below,
                        // as the mark this same person registered.
                        "© {copyright_year} {copyright_holder}"
                    }
                    // The other ownership fact, directly under the first: the
                    // wordmark this footer opens on is a registered mark, and
                    // this is who holds it and under what number. It sits above
                    // the bar licences because it is a property notice about
                    // the site rather than regulated attorney copy, and it
                    // links out for the same reason those rows do — a reader
                    // checks the register, not the page's word for it.
                    if !trademark.is_empty() && !trademark_registration.is_empty() {
                        p { class: "site-footer__trademark",
                            "{trademark}"
                            // `line-height: 0` in the stylesheet keeps the
                            // superscript from stretching the fine print's
                            // line box; the glyph is part of the mark, not
                            // decoration, so it stays in the text.
                            sup { class: "site-footer__trademark-mark", "®" }
                            " is a registered trademark of {copyright_holder}, "
                            // The notice ends on the registration, with no
                            // closing period — the same shape as each bar row
                            // below. `ExternalLink` closes on its
                            // leaving-the-site glyph, so punctuation after it
                            // would set adrift of the number it belongs to.
                            if trademark_record_url.is_empty() {
                                "U.S. Reg. No. {trademark_registration}"
                            } else {
                                ExternalLink {
                                    href: trademark_record_url.clone(),
                                    class: "link-secondary".to_string(),
                                    "U.S. Reg. No. {trademark_registration}"
                                }
                            }
                        }
                    }
                    // Who is licensed, where, and under what number — each
                    // linked to the bar's own record so a visitor can verify
                    // the licence rather than take the site's word for it. This
                    // is the footer's only bar disclosure: a firm-level
                    // "Admitted in …" line said nothing these rows do not, in
                    // the jurisdictions they already name.
                        if !attorneys.is_empty() {
                            ul { class: "site-footer__licenses",
                                for attorney in attorneys.iter() {
                                    li { class: "site-footer__licensee", key: "{attorney.name}",
                                        span { class: "site-footer__licensee-name", "{attorney.name}" }
                                        " — "
                                        for (index, license) in attorney.licenses.iter().enumerate() {
                                            if index > 0 {
                                                " · "
                                            }
                                            ExternalLink {
                                                href: license.license_url.clone(),
                                                class: "link-secondary".to_string(),
                                                "{license.jurisdiction} Bar No. {license.number}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        p { class: "site-footer__disclaimer", "{disclaimer}" }
                    }
                    // The repository the platform is developed in and the
                    // release running here, closing the strip on one line. No
                    // box, no attribution prose — just the repository's name
                    // and its star count, the way the rest of the site links
                    // off to GitHub, with the version right beside it rather
                    // than on a line of its own.
                    //
                    // Each half stands alone: a deploy publishes the
                    // repository without a release stamp under `cargo run`,
                    // and the region itself renders only when there is
                    // something to put in it.
                    if (!source_repo.is_empty() && !source_href.is_empty())
                        || (!navigator_version.is_empty() && !navigator_href.is_empty())
                    {
                        div { class: "site-footer__legal-platform",
                            p { class: "site-footer__source",
                                if !source_repo.is_empty() && !source_href.is_empty() {
                                    GitHubStars {
                                        href: source_href.clone(),
                                        repo: source_repo.clone(),
                                        stars: source_stars,
                                    }
                                }
                                if !navigator_version.is_empty() && !navigator_href.is_empty() {
                                    if !source_repo.is_empty() && !source_href.is_empty() {
                                        span {
                                            class: "site-footer__release-sep",
                                            "aria-hidden": "true",
                                            "\u{b7}"
                                        }
                                    }
                                    a { class: "site-footer__release", href: "{navigator_href}",
                                        "#{navigator_version}"
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

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    /// The link row a deploy hands this component, mirroring what
    /// `views::brand::firm_footer_nav` publishes: the eight public routes the
    /// header does not carry, alphabetized by label.
    const FOOTER_ROW: [(&str, &str); 8] = [
        ("Blog", "/blog"),
        ("Docs", "/docs"),
        ("Navigator", "/navigator"),
        ("Notations", "/notations"),
        ("Presentations", "/presentations"),
        ("Privacy", "/privacy"),
        ("Terms", "/terms"),
        ("Workshops", "/workshops"),
    ];

    /// The firm's legal strip: the copyright line that names the entity, the
    /// attorney-advertising disclaimer, and nothing else the deploy did not
    /// hand in.
    fn legal_html() -> String {
        fn app() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                    trademark: "NEON LAW".to_string(),
                    trademark_registration: "6,325,650".to_string(),
                    trademark_record_url:
                        "https://tmsearch.uspto.gov/search/search-results/90039224".to_string(),
                    source_repo: "neon-law-source-code/navigator".to_string(),
                    source_href: "https://github.com/neon-law-source-code/navigator".to_string(),
                    source_stars: 1234u64,
                }
            }
        }
        ssr(app)
    }

    /// The mark notice names the mark, the registrant, and the number, and
    /// links the number to the register's own record.
    ///
    /// Order is the substance: the two ownership facts read together, so the
    /// notice follows the copyright and precedes the bar licences.
    #[test]
    fn notices_the_registered_mark_under_the_copyright() {
        let out = legal_html();
        assert!(
            out.contains("NEON LAW") && out.contains("®"),
            "the mark renders as the register spells it, with its symbol: {out}"
        );
        assert!(
            out.contains("is a registered trademark of Neon Law"),
            "and names the registrant the copyright line names: {out}"
        );
        assert!(
            out.contains("U.S. Reg. No. 6,325,650"),
            "the registration is the claim: {out}"
        );
        assert!(
            out.contains(r#"href="https://tmsearch.uspto.gov/search/search-results/90039224""#),
            "linked to the register's own record: {out}"
        );
        let copyright = out.find("site-footer__copyright").expect("the copyright");
        let mark = out.find("site-footer__trademark").expect("the mark notice");
        let licences = out.find("site-footer__licenses").unwrap_or(usize::MAX);
        assert!(
            copyright < mark && mark < licences,
            "the ownership facts read together, above the bar rows: {out}"
        );
    }

    /// A deploy holding no registration notices none, and one that cites a
    /// number with no public record renders the claim unlinked rather than
    /// pointing at nothing.
    #[test]
    fn omits_the_mark_notice_when_unregistered_and_the_link_when_unrecorded() {
        fn unregistered() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Cascade Law LLP".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                }
            }
        }
        fn unrecorded() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                    trademark: "NEON LAW".to_string(),
                    trademark_registration: "6,325,650".to_string(),
                }
            }
        }

        let out = ssr(unregistered);
        assert!(
            !out.contains("site-footer__trademark") && !out.contains("registered trademark"),
            "no registration, no notice: {out}"
        );

        let out = ssr(unrecorded);
        assert!(
            out.contains("U.S. Reg. No. 6,325,650"),
            "the claim still renders: {out}"
        );
        assert!(
            !out.contains("tmsearch"),
            "with no link to a record it does not have: {out}"
        );
    }

    /// The open-source line names the repository, links it, and prints the
    /// star count — and it closes the strip, below the disclaimer.
    ///
    /// Order is the substance here, as it is for every other line in this
    /// strip: the repository is a developer surface, so a reader meets the bar
    /// records and the advertising disclaimer before the page mentions where
    /// the code lives.
    #[test]
    fn closes_the_strip_with_the_source_repository_and_its_stars() {
        let out = legal_html();
        assert!(
            out.contains(r#"href="https://github.com/neon-law-source-code/navigator""#),
            "the repository is linked: {out}"
        );
        assert!(
            out.contains("github-stars__repo") && out.contains("neon-law-source-code/navigator"),
            "and named as the project's source: {out}"
        );
        assert!(
            out.contains("1,234") && out.contains("<title>GitHub stars</title>"),
            "the star count renders under its own accessible name: {out}"
        );
        let disclaimer = out.find("attorney advertisement").expect("the disclaimer");
        let source = out.find("site-footer__source").expect("the source line");
        assert!(
            disclaimer < source,
            "the source line closes the strip, under the disclaimer: {out}"
        );
    }

    /// The running release sits right beside the repository link, on the same
    /// line, rather than under it as a line of its own.
    #[test]
    fn sets_the_release_beside_the_repository_on_one_line() {
        let out = contactable_html();
        assert!(
            out.contains("#26.8.20"),
            "the version renders next to the repository: {out}"
        );
        assert!(
            !out.contains("Neon Law Navigator #"),
            "the release no longer carries its own sentence: {out}"
        );
        assert_eq!(
            out.matches(r#"class="site-footer__source""#).count(),
            1,
            "one line carries both the repository and the release: {out}"
        );
        assert!(
            !out.contains(r#"<p class="site-footer__release""#),
            "the release is no longer its own paragraph: {out}"
        );
        let repo = out
            .find("neon-law-source-code/navigator")
            .expect("the repo");
        let version = out.find("#26.8.20").expect("the version");
        assert!(
            repo < version,
            "the version follows the repository it describes: {out}"
        );
    }

    /// A deploy that publishes no repository renders no line, and one whose
    /// star count has not been fetched yet renders the link without a number.
    ///
    /// The second half is the ordinary case, not an edge one: the count comes
    /// from a cache a background task fills after boot, so every render before
    /// the first fetch — and every render in a process that never spawned the
    /// refresh, which is every test — takes this path.
    #[test]
    fn omits_the_source_line_when_unset_and_the_count_when_unfetched() {
        fn app() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                }
            }
        }
        fn unfetched() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                    source_repo: "neon-law-source-code/navigator".to_string(),
                    source_href: "https://github.com/neon-law-source-code/navigator".to_string(),
                }
            }
        }

        let out = ssr(app);
        assert!(
            !out.contains("site-footer__source"),
            "no repository, no line: {out}"
        );
        assert!(!out.contains(r#"href="""#), "no empty anchor: {out}");

        let out = ssr(unfetched);
        assert!(
            out.contains(r#"href="https://github.com/neon-law-source-code/navigator""#),
            "an unknown count still publishes the repository: {out}"
        );
        assert!(
            !out.contains("GitHub stars"),
            "and prints no number in place of one it does not have: {out}"
        );
    }

    /// The copyright names one entity: the firm's legal person, and nobody
    /// else.
    ///
    /// It used to name a second organization beside it, when the site served a
    /// nonprofit's pages as well as the firm's. That surface is retired, and a
    /// copyright line crediting an organization whose pages the site no longer
    /// publishes would be a claim about ownership that is no longer true.
    #[test]
    fn the_copyright_names_the_firms_legal_person_alone() {
        let out = legal_html();
        let copyright = out
            .split(r#"<p class="site-footer__copyright">"#)
            .nth(1)
            .and_then(|rest| rest.split("</p>").next())
            .expect("the copyright line renders");
        assert!(
            copyright.contains("\u{a9} 2026 Neon Law"),
            "the copyright names the holding entity: {copyright}"
        );
        assert!(
            !copyright.contains("Foundation"),
            "the copyright names one organization: {copyright}"
        );
        assert!(
            !copyright.contains(" and "),
            "and names it without a second: {copyright}"
        );
    }

    /// The copyright heads the legal strip and there is exactly one of it.
    ///
    /// Asserted on the copyright element rather than on the entity's name. The
    /// name now appears twice on purpose: the supporter line at the foot of the
    /// page says "Neon Law is a proud supporter of …", which is the
    /// firm's own wording. What must not recur is the *copyright*, which once
    /// trailed a firm-level attribution line saying the same thing.
    #[test]
    fn renders_the_copyright_once_at_the_head_of_the_strip() {
        let out = legal_html();
        assert_eq!(
            out.matches(r#"class="site-footer__copyright""#).count(),
            1,
            "one copyright line: {out}"
        );
        let legal = out
            .find(r#"<div class="site-footer__legal""#)
            .expect("the legal strip renders");
        let copyright = out
            .find(r#"<p class="site-footer__copyright""#)
            .expect("the copyright renders");
        let disclaimer = out
            .find(r#"<p class="site-footer__disclaimer""#)
            .expect("the disclaimer renders");
        assert!(
            legal < copyright && copyright < disclaimer,
            "the copyright heads the strip: {out}"
        );
    }

    /// The firm-level "Admitted in California \u{b7} Washington \u{b7} Nevada" line is
    /// gone. A jurisdiction is published per attorney, with the bar number, by
    /// the licence list below — never as a firm-wide claim.
    #[test]
    fn publishes_no_firm_level_admissions_line() {
        let out = legal_html();
        assert!(
            !out.contains("Admitted in"),
            "the firm-level admissions line is retired: {out}"
        );
        let licensed = contactable_html();
        assert!(
            licensed.contains("Nevada Bar No."),
            "the jurisdiction is still published as an attorney licence: {licensed}"
        );
    }

    /// A footer carrying every contact channel: the CTA, the voice line, the
    /// offices, and the per-attorney bar licenses.
    fn contactable_html() -> String {
        fn app() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                    logo_href: "/public/logo.svg".to_string(),
                    brand_name: "Neon Law".to_string(),
                    contact_email: "support@neonlaw.com".to_string(),
                    phone: "+1 510 800 2080".to_string(),
                    // The real addresses, as `views::brand` publishes them: a
                    // suite is its own comma-separated part, which is what the
                    // footer breaks a line on.
                    offices: [
                        ("California", "1990 N California Blvd, Ste 800, Walnut Creek, CA 94596", None),
                        ("Nevada", "5150 Mae Anne Ave, Ste 405-9777, Reno, NV 89523", None),
                        (
                            "New York",
                            "12 E 49th St, 18th Floor, New York, NY 10017",
                            Some("Bar admission pending"),
                        ),
                        ("Washington", "720 Seneca St, Ste 107-715, Seattle, WA 98101", None),
                    ]
                    .into_iter()
                    .map(|(state, address, note)| FooterOffice {
                        state: state.to_string(),
                        address: address.to_string(),
                        note: note.map(str::to_string),
                    })
                    .collect(),
                    attorneys: vec![
                        FooterAttorney {
                            name: "Nicholas Richard Shook".to_string(),
                            licenses: vec![FooterBarLicense {
                                jurisdiction: "Nevada".to_string(),
                                number: "13400".to_string(),
                                license_url: "https://nvbar.org/find-a-lawyer/?usearch=13400"
                                    .to_string(),
                            }],
                        },
                    ],
                    nav: FOOTER_ROW
                        .into_iter()
                        .map(|(label, href)| FooterNavLink {
                            label: label.to_string(),
                            href: href.to_string(),
                        })
                        .collect(),
                    source_repo: "neon-law-source-code/navigator".to_string(),
                    source_href: "https://github.com/neon-law-source-code/navigator".to_string(),
                    navigator_version: "26.8.20".to_string(),
                    navigator_href: "/navigator".to_string(),
                }
            }
        }
        ssr(app)
    }

    /// The footer's content sits in the same width-capped column the header nav
    /// uses, so it lines up with the navbar rather than running to the edge.
    #[test]
    fn wraps_its_content_in_the_navbar_aligned_column() {
        let out = contactable_html();
        assert!(
            out.contains(r#"<footer class="site-footer""#),
            "the rule/background element is the outer footer: {out}"
        );
        let inner = out
            .find(r#"<div class="site-footer__inner""#)
            .expect("the width-capped column is present");
        let legal = out.find("site-footer__legal").expect("legal strip present");
        assert!(
            inner < legal,
            "the legal strip sits inside the column: {out}"
        );
        for region in ["site-footer__legal-practice", "site-footer__legal-platform"] {
            assert!(
                out.contains(region),
                "the legal copy has a {region} region: {out}"
            );
        }
    }

    #[test]
    fn renders_the_contact_cta_and_voice_line() {
        let out = contactable_html();
        assert!(
            out.contains(r#"class="site-footer__logo" src="/public/logo.svg" alt="""#),
            "the supplied brand mark renders as decorative footer identity: {out}"
        );
        // The subject rides the address, so the firm reads what the sender
        // wants from the message list.
        assert!(
            out.contains(r#"href="mailto:support@neonlaw.com?subject=Legal%20services.""#),
            "the CTA mails the firm's inbound address under a named subject: {out}"
        );
        // `tel:` dials digits only — the human spacing would not dial.
        assert!(
            out.contains(r#"href="tel:+15108002080""#),
            "the voice line is dialable: {out}"
        );
        assert!(
            out.contains("+1 510 800 2080"),
            "the number is shown as written: {out}"
        );
    }

    /// Each office is labelled by its state and they render in the given
    /// order. The assertion anchors on the street addresses rather than on the
    /// labels: "Walnut Creek" and "Seattle" still occur inside their own
    /// addresses, so a label-only check would pass even if the labels had never
    /// changed.
    #[test]
    fn lists_every_office_in_order() {
        let out = contactable_html();
        let positions: Vec<usize> = [
            "1990 N California Blvd",
            "5150 Mae Anne Ave",
            "12 E 49th St",
            "720 Seneca St",
        ]
        .iter()
        .map(|address| {
            out.find(address)
                .unwrap_or_else(|| panic!("{address} present: {out}"))
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "offices render in the given order: {out}"
        );
        for label in ["California", "Nevada", "New York", "Washington"] {
            assert!(
                out.contains(&format!(r#"class="site-footer__office-label">{label}<"#)),
                "{label} labels its office: {out}"
            );
        }
    }

    /// Each address is set line by line — street, unit, then city with its state
    /// and ZIP — so the suite has its own line and the city starts one instead
    /// of landing wherever the narrow footer column happened to wrap.
    #[test]
    fn sets_each_address_over_a_street_a_unit_and_a_city_line() {
        let out = contactable_html();
        for (street, unit, city) in [
            (
                "1990 N California Blvd",
                "Ste 800",
                "Walnut Creek, CA 94596",
            ),
            ("5150 Mae Anne Ave", "Ste 405-9777", "Reno, NV 89523"),
            ("12 E 49th St", "18th Floor", "New York, NY 10017"),
            ("720 Seneca St", "Ste 107-715", "Seattle, WA 98101"),
        ] {
            let line =
                |text: &str| format!(r#"<span class="site-footer__office-line">{text}</span>"#);
            assert!(
                out.contains(&format!("{}{}{}", line(street), line(unit), line(city))),
                "{street} / {unit} / {city} are three lines: {out}"
            );
        }
        assert_eq!(
            out.matches("site-footer__office-line").count(),
            12,
            "three lines for each of the four offices: {out}"
        );
    }

    /// The city keeps its state and ZIP, every other comma is a break, and an
    /// address with no city to lift out publishes as the one line it was written
    /// as rather than broken at a guess.
    #[test]
    fn breaks_on_every_comma_but_the_one_holding_a_city_to_its_state() {
        assert_eq!(
            super::address_lines("1990 N California Blvd, Ste 800, Walnut Creek, CA 94596"),
            [
                "1990 N California Blvd",
                "Ste 800",
                "Walnut Creek, CA 94596"
            ],
        );
        // A firm that writes its address on one line still gets a city line.
        assert_eq!(
            super::address_lines("1 Main St, Boise, ID 83702"),
            ["1 Main St", "Boise, ID 83702"],
        );
        // Four lines above the city, if that is how the address is written.
        assert_eq!(
            super::address_lines("Attn: Mail, Bldg 4, Ste 400, 1 Main St, Boise, ID 83702"),
            [
                "Attn: Mail",
                "Bldg 4",
                "Ste 400",
                "1 Main St",
                "Boise, ID 83702"
            ],
        );
        // A city with no state beside it is still a line of its own.
        assert_eq!(
            super::address_lines("1 Main St, Boise"),
            ["1 Main St", "Boise"],
        );
        // Nothing to break: published as written.
        assert_eq!(
            super::address_lines("General Delivery"),
            ["General Delivery"]
        );
        assert!(super::address_lines("").is_empty(), "no address, no line");
    }

    /// Every published state carries its outline, and the outline is
    /// decoration rather than content: it is `aria-hidden`, so a screen reader
    /// hears the state once from the label rather than twice.
    #[test]
    fn draws_each_state_behind_its_own_office() {
        let out = contactable_html();
        assert_eq!(
            out.matches("site-footer__office-map").count(),
            4,
            "one watermark per office: {out}"
        );
        for state in ["California", "Nevada", "New York", "Washington"] {
            assert!(
                super::state_outline(state).is_some(),
                "{state} has a traced outline"
            );
        }
        // The SVG is hidden from assistive technology and unfocusable.
        assert!(
            out.contains(r#"focusable="false""#),
            "not a tab stop: {out}"
        );
        // Colour comes from the stylesheet, never from the component — the
        // outline inherits it so one token themes all four.
        assert!(
            out.contains(r#"fill="currentColor""#),
            "the outline inherits its colour: {out}"
        );
    }

    /// Every point of an outline, read out of its `d` attribute.
    fn outline_points(state: &str) -> Vec<(f64, f64)> {
        let d = super::state_outline(state).expect("the state has an outline");
        let numbers: Vec<f64> = d
            .replace(['M', 'Z'], " ")
            .split_whitespace()
            .map(|token| token.parse::<f64>().expect("a coordinate"))
            .collect();
        assert_eq!(numbers.len() % 2, 0, "{state} has whole coordinate pairs");
        numbers.chunks(2).map(|pair| (pair[0], pair[1])).collect()
    }

    /// The outlines are the real state geometry, fitted at true aspect ratio —
    /// not four silhouettes stretched to fill a square.
    ///
    /// This is the whole substance of the tracing: a border scaled unequally on
    /// its two axes stops being that state's shape no matter how faithful its
    /// corners are, and it is exactly the failure a hand-drawn replacement
    /// reintroduces. So each outline has to fill the padded box on its long
    /// axis, fall short of it on the short one, and stand the way the state
    /// actually stands.
    #[test]
    fn fits_each_outline_at_the_state_real_aspect_ratio() {
        // Portrait or landscape, as the state itself is: California and Nevada
        // run north–south, New York and Washington east–west.
        for (state, landscape) in [
            ("California", false),
            ("Nevada", false),
            ("New York", true),
            ("Washington", true),
        ] {
            let points = outline_points(state);
            let width = points.iter().map(|p| p.0).fold(f64::MIN, f64::max)
                - points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
            let height = points.iter().map(|p| p.1).fold(f64::MIN, f64::max)
                - points.iter().map(|p| p.1).fold(f64::MAX, f64::min);
            assert!(
                points
                    .iter()
                    .all(|(x, y)| (0.0..=100.0).contains(x) && (0.0..=100.0).contains(y)),
                "{state} stays inside the 100×100 box"
            );
            assert!(
                (width.max(height) - 94.0).abs() < 1.0,
                "{state} fills the padded box on its long axis: {width}×{height}"
            );
            assert!(
                width.min(height) < 90.0,
                "{state} is fitted, not stretched square: {width}×{height}"
            );
            assert_eq!(
                width > height,
                landscape,
                "{state} stands the way the state does: {width}×{height}"
            );
        }
    }

    /// The routes the header does not carry live in the footer, so the footer
    /// has to actually link them. A route dropped from the nav and not picked
    /// up here is a page reachable only by typing its URL.
    #[test]
    fn links_the_pages_the_header_no_longer_carries() {
        let out = contactable_html();
        for (label, href) in FOOTER_ROW {
            assert!(
                out.contains(&format!(r#"href="{href}""#)),
                "the footer links {label}: {out}"
            );
        }
        assert!(
            out.contains(r#"aria-label="More pages""#),
            "the row is a labelled landmark: {out}"
        );
    }

    /// The row is one list, at every width.
    ///
    /// Two columns of five on a wide viewport and one column of ten on a narrow
    /// one is a stylesheet job — `.site-footer__nav-list` flows down five rows
    /// and across, and the narrow layout stops columnizing it. Rendering two
    /// `<ul>`s to get the wide layout would give a screen reader two lists of
    /// five on every viewport and split the reading order down the middle, so
    /// this asserts the single list the CSS is written against.
    #[test]
    fn renders_the_row_as_one_list_of_sibling_destinations() {
        let out = contactable_html();
        let row = out
            .split_once(r#"aria-label="More pages""#)
            .and_then(|(_, rest)| rest.split_once("</nav>"))
            .map(|(row, _)| row)
            .expect("the row renders as a labelled landmark");
        assert_eq!(
            row.matches(r#"<ul class="site-footer__nav-list""#).count(),
            1,
            "one list, whatever the stylesheet does with it: {row}"
        );
        assert_eq!(
            row.matches(r#"<li class="site-footer__nav-item""#).count(),
            FOOTER_ROW.len(),
            "every destination is an item of it: {row}"
        );
    }

    /// The links are plain text, not pills.
    ///
    /// The row carries the site's remaining pages, so it reads as a directory
    /// in the footer's own muted type. The pill treatment it used to wear —
    /// a border, a radius, a hover fill — made ten quiet links look like ten
    /// buttons competing with the contact band's actual calls to action. The
    /// styling lives in `.site-footer__nav-link`, so this asserts the markup
    /// hands the stylesheet nothing else to hook.
    #[test]
    fn the_row_carries_no_styling_of_its_own() {
        let out = contactable_html();
        let row = out
            .split_once(r#"aria-label="More pages""#)
            .and_then(|(_, rest)| rest.split_once("</nav>"))
            .map(|(row, _)| row)
            .expect("the row renders as a labelled landmark");
        assert!(
            !row.contains("style="),
            "no inline styling on the row: {row}"
        );
        assert_eq!(
            row.matches(r#"class="site-footer__nav-link""#).count(),
            FOOTER_ROW.len(),
            "every link wears the one class and no variant: {row}"
        );
    }

    /// A deploy that publishes no footer routes renders no empty row.
    #[test]
    fn omits_the_nav_row_when_unset() {
        fn app() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                }
            }
        }
        let out = ssr(app);
        assert!(!out.contains("site-footer__nav"), "no empty row: {out}");
    }

    /// The footer names the release serving the page, right beside the
    /// repository it is built from, on the one line that closes the strip.
    ///
    /// This is what makes a push visible end to end: the moment a new image is
    /// live, the number at the bottom of every public page changes. It links
    /// `/navigator`, the page describing the platform, so a reader who wants to
    /// know what the number refers to has somewhere to go.
    #[test]
    fn publishes_the_release_it_is_running() {
        let out = contactable_html();
        assert!(
            out.contains("#26.8.20"),
            "the footer names the running release: {out}"
        );
        assert!(
            out.contains(r#"<a class="site-footer__release" href="/navigator">"#),
            "linked to the page describing the platform: {out}"
        );
        let platform = out
            .find("site-footer__legal-platform")
            .expect("the platform region renders");
        let source = out.find("site-footer__source").expect("the source line");
        let repo = out
            .find("neon-law-source-code/navigator")
            .expect("the repository renders");
        let release = out.find("#26.8.20").expect("the release renders");
        assert!(
            platform < source && repo < release,
            "the release sits beside the repository it is built from: {out}"
        );
    }

    /// An unstamped build publishes no version.
    ///
    /// `NAVIGATOR_RELEASE_TAG` is unset under a local `cargo run`, and a footer
    /// reading "#" is worse than no attribution. The repository line is
    /// independent and still renders, and with both halves absent the region
    /// itself disappears rather than leaving an empty box.
    #[test]
    fn omits_the_release_line_when_unpublished() {
        fn repository_only() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                    source_repo: "neon-law-source-code/navigator".to_string(),
                    source_href: "https://github.com/neon-law-source-code/navigator".to_string(),
                }
            }
        }
        fn neither() -> Element {
            rsx! {
                SiteFooterLegal {
                    copyright_holder: "Neon Law".to_string(),
                    disclaimer: "This is an attorney advertisement.".to_string(),
                    copyright_year: 2026,
                }
            }
        }
        let out = ssr(repository_only);
        assert!(
            !out.contains("site-footer__release"),
            "no release, no version link: {out}"
        );
        assert!(
            out.contains("site-footer__source"),
            "the repository line is independent of it: {out}"
        );
        let bare = ssr(neither);
        assert!(
            !bare.contains("site-footer__legal-platform"),
            "with neither half, the region itself does not render: {bare}"
        );
    }

    /// A deploy that publishes an office in a state this component carries no
    /// tracing of renders the plain treatment rather than a stray box.
    #[test]
    fn omits_the_watermark_for_an_untraced_state() {
        assert!(super::state_outline("Idaho").is_none());
        assert!(super::state_outline("").is_none());
    }

    /// An office note is a qualification on one specific address, so it must
    /// render inside that office's own `<li>` — between the address it
    /// qualifies and the next city. A note that escapes into the neighbouring
    /// entry attaches a pending admission to the wrong jurisdiction.
    #[test]
    fn renders_an_office_note_beneath_the_address_it_qualifies() {
        let out = contactable_html();
        // The last line of the New York address, so the note has to follow the
        // whole of it rather than slotting between two of its lines.
        let address = out
            .find("New York, NY 10017")
            .expect("the New York address renders");
        let note = out
            .find("Bar admission pending")
            .expect("the New York note renders");
        let next_office = out.find("720 Seneca St").expect("the next office renders");
        assert!(
            address < note && note < next_office,
            "the note sits under its own address, before the next office: {out}"
        );
        assert!(
            out.contains(r#"class="site-footer__office-note""#),
            "the note is styled as a qualification rather than a line of the address: {out}"
        );
        // The unqualified offices publish no note element at all, rather than
        // an empty one that reserves space under every address.
        assert_eq!(
            out.matches("site-footer__office-note").count(),
            1,
            "only the qualified office renders a note: {out}"
        );
    }

    /// The bar number is the point of the licence line: it must render beside
    /// its jurisdiction and link to that bar's own record, so a visitor can
    /// verify the licence rather than trust the page.
    #[test]
    fn names_each_attorney_with_their_bar_number_and_record() {
        let out = contactable_html();
        assert!(
            out.contains("Nicholas Richard Shook"),
            "the licensed attorney is named: {out}"
        );
        assert!(
            out.contains("Nevada Bar No. 13400"),
            "the bar number renders beside its jurisdiction: {out}"
        );
        assert!(
            out.contains("nvbar.org/find-a-lawyer/?usearch=13400"),
            "the number links to the bar's own record: {out}"
        );
    }

    /// A deploy that publishes no contact channels renders no empty band — and
    /// its legal strip is still the footer's first child, so the CSS drops the
    /// second rule.
    #[test]
    fn omits_the_contact_band_when_nothing_is_published() {
        let out = legal_html();
        assert!(
            !out.contains("site-footer__contact"),
            "no contact band without contact details: {out}"
        );
        assert!(
            !out.contains("site-footer__licenses"),
            "no licence list without attorneys: {out}"
        );
        assert!(
            out.contains(r#"<div class="site-footer__legal""#),
            "the legal strip still renders: {out}"
        );
    }
}
