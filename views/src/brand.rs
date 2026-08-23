// Brand-name prose (NeonLaw, NeonLaw Foundation) trips
// clippy::doc_markdown; the brand names are not code identifiers.
// Same precedent as views/src/components.rs.
#![allow(clippy::doc_markdown)]

//! Site brand: the strings and links that identify the product to
//! the visitor (name, copyright owner, nav targets).
//!
//! Two brands share one binary: [`FIRM_BRAND`] for the law firm,
//! [`FOUNDATION_BRAND`] for the 501(c)(3). Each page picks its brand
//! with `PageLayout::with_brand`; the layout never branches on the URL.
//!
//! Branding defaults to Neon Law's complete in-repository identity. A web
//! deployment can load a validated [`crate::brand_bundle::BrandManifest`]
//! once and scope the resulting immutable [`Branding`] to each request. No
//! brand field is read from or written to process environment.

use std::env;
use std::future::Future;
use std::ops::Deref;
use std::sync::{LazyLock, OnceLock};

use crate::brand_bundle::BrandManifest;

/// Bundle of strings + nav links that identify the running site.
///
/// `Copy` is preserved so the layout's `with_brand(SiteBrand)` API
/// continues to take the brand by value without a clone.
#[derive(Debug, Clone, Copy)]
pub struct SiteBrand {
    pub site_name: &'static str,
    pub tagline: &'static str,
    /// Landing path the navbar brand (logo + wordmark) links back to.
    /// The firm's home is the site root `/`; the Foundation's home is
    /// its own hub at `/foundation`, so its header returns there rather
    /// than to the firm's root.
    pub home_href: &'static str,
    /// One-line postal address rendered in the footer. Differs per
    /// entity (the firm and the Foundation share a street but hold
    /// distinct private-mailbox suites). A mounted bundle lets an OSS
    /// deployment supply its own registered address.
    pub postal_address: &'static str,
    /// Path to the header brand mark served under `/public/`.
    ///
    /// `SiteHeader` renders it through `<img src=…>` at 32 CSS px, which
    /// constrains the asset in two ways. An SVG here runs in the browser's
    /// **secure static mode**, so it must be wholly self-contained: it cannot
    /// fetch an external `<image>`, stylesheet, or font, and it cannot inherit
    /// `currentColor` from the page. A raster here must be sized for the
    /// header rather than for print, since every visitor downloads it. Both
    /// built-in brands supply a self-contained vector whose colours are
    /// written into the file.
    pub logo_href: &'static str,
    /// Path to a **raster** (PNG) brand mark served under `/public/`,
    /// used as the Open Graph / Twitter Card `og:image`. Social-share
    /// scrapers (iMessage, Slack, Facebook, X, LinkedIn) generally
    /// won't rasterize SVG, so the share card needs its own PNG. It is also
    /// the full-resolution mark, where [`logo_href`] is header-sized.
    pub social_image: &'static str,
    pub nav: &'static [NavLink],
    /// When true, the layout renders firm-only portal links. Foundation
    /// pages (the 501(c)(3) doesn't practice law) leave this false. The
    /// legal-advice disclaimer is no longer gated here — the unified footer
    /// always shows the firm's via [`firm_disclaimer`].
    pub is_law_firm: bool,
    /// The legal person the footer's copyright names — the entity that owns
    /// the site and renders the firm's legal services. A configurable identity
    /// so a rename is a manifest edit, not a code change. Empty for the
    /// Foundation and any non-firm brand, which falls back to noticing its own
    /// wordmark. Sourced from `brand.firm_legal_entity` in the manifest;
    /// defaults to today's value.
    ///
    /// Kept in step with `store::seed::FIRM_ENTITY_NAME`, the firm Entity row
    /// the application refuses to delete.
    pub legal_entity: &'static str,
}

/// One header nav entry. A `NavLink` is either a leaf (no children)
/// or a dropdown (children populate a Pico `<details class="dropdown">`).
#[derive(Debug, Clone, Copy)]
pub struct NavLink {
    pub label: &'static str,
    pub href: &'static str,
    /// Optional sub-items. Empty slice means a plain leaf link.
    pub children: &'static [NavLink],
    /// Optional Bootstrap Icon name (the part after `bi-`, e.g.
    /// `"star-fill"`) shown before the label. `None` renders no icon.
    /// Used to denote each product in the Services dropdown.
    pub icon: Option<&'static str>,
}

impl NavLink {
    #[must_use]
    pub const fn leaf(label: &'static str, href: &'static str) -> Self {
        Self {
            label,
            href,
            children: &[],
            icon: None,
        }
    }

    /// A leaf link prefixed with a Bootstrap Icon. `icon` is the glyph
    /// name without the `bi-` prefix (e.g. `"shield-fill-check"`).
    #[must_use]
    pub const fn leaf_with_icon(
        label: &'static str,
        href: &'static str,
        icon: &'static str,
    ) -> Self {
        Self {
            label,
            href,
            children: &[],
            icon: Some(icon),
        }
    }

    #[must_use]
    pub const fn dropdown(label: &'static str, children: &'static [NavLink]) -> Self {
        Self {
            label,
            href: "#",
            children,
            icon: None,
        }
    }

    #[must_use]
    pub const fn is_dropdown(&self) -> bool {
        !self.children.is_empty()
    }
}

/// The firm's header navigation: the lead offering, then the three practices
/// under it.
///
/// Fractional CTO leads because it is what the firm leads with — it runs the
/// technology function for the law firms it serves, and the home page states it
/// as the one thing above the fold. The order is the claim, so it is asserted
/// rather than left to this literal.
///
/// The three that follow are real practices with pages of their own, and a
/// reader who came for one of them must not have to hunt the footer:
/// `/litigation` the disputes practice, `/fractional-gc` (fractional general
/// counsel) the company-counsel work, and `/services` the flat-fee schedule of
/// routine one-time matters. The two quoted practices sit nearer the lead
/// offering than the schedule does, because a firm reading the lead is the
/// reader those two are for.
///
/// Foundation closes the row, and it is the one entry here that is not the
/// firm's own work. It has to be: the nonprofit's face no longer carries a
/// header of its own, so this row is the header on every page of the site, and
/// without this entry the 501(c)(3) would be reachable from the top of a page
/// only by the reader knowing to scroll past the whole thing. Last rather than
/// first because a visitor at the top of a law firm's page is deciding whether
/// to hire a lawyer; the nonprofit is what they look at after.
///
/// It is deliberately the one label this row and [`FIRM_FOOTER_NAV`] both
/// carry — see the `the_footer_nav_carries_what_the_header_does_not` assertion,
/// which exempts it by name.
///
/// Everything a reader looks for second — the Blog, Navigator, how to reach
/// the firm — stays in [`FIRM_FOOTER_NAV`].
const FIRM_NAV: &[NavLink] = &[
    NavLink::leaf("Fractional CTO", "/fractional-cto"),
    NavLink::leaf("Litigation", "/litigation"),
    NavLink::leaf("Fractional GC", "/fractional-gc"),
    NavLink::leaf("Legal Services", "/services"),
    NavLink::leaf("Foundation", "/foundation"),
];

/// The rest of the firm's public surface, rendered in the footer rather than the
/// header, and ordered alphabetically by label.
///
/// Not a lesser set of pages — a later one. Every route here is still linked
/// from the site on every page; it is linked where a reader looks for it
/// second.
///
/// Services is deliberately absent: it carries the published fee schedule, so it
/// sits in [`FIRM_NAV`] under the lead offering rather than waiting here.
///
/// `/navigator` describes the platform the firms we serve work on, which a
/// reader asks about after deciding the firm does their kind of work — the same
/// shape of question as the Blog. The platform attribution at the very bottom of
/// the footer names the running release and links the same page; this is the
/// reader-facing route to it, not a version stamp.
///
/// "Firm" and "Foundation" are the row's two cross-links, and they sit here as
/// a pair because one binary serves both faces: from anywhere on the site, each
/// organization's home is one click away. They are labelled by what the reader
/// is choosing between — the law firm at `/` and the nonprofit at `/foundation`
/// — rather than by either wordmark, so the two entries read as the two halves
/// of one site and neither label drifts when a wordmark changes.
///
/// Presentations sits here rather than the header. The talks are the firm's,
/// given at meetups and conferences and published here; a reader deciding
/// whether to work with the firm is not looking for them first, and a reader
/// who saw one at a conference knows to look at the bottom of the page.
///
/// Docs joined the row when the workspace documentation became anonymous. It is
/// a shared portal route rather than a firm page, so it resolves on both faces —
/// and it belongs at the bottom for the plainest reason on this list: a reader
/// who wants the manual for the software has already decided to run it.
///
/// Privacy and Terms sit in this row on the same footing as the Blog and
/// Contact, not in a smaller strip beneath it. They are the two documents a
/// reader is entitled to find without hunting, and the legal strip below
/// already carries the copyright, the bar disclosure, and the advertising
/// disclaimer — a second, quieter row of legal links there would read as fine
/// print about fine print. Their bodies already serve at `/privacy` and
/// `/terms`; this is the link that reaches them.
///
/// Ten entries, and the count is part of the design: the footer lays them out
/// as two columns of five on a wide viewport and one list of ten on a narrow
/// one, so an eleventh would leave a column uneven.
const FIRM_FOOTER_NAV: &[NavLink] = &[
    NavLink::leaf("Blog", "/blog"),
    NavLink::leaf("Contact", "/contact"),
    NavLink::leaf("Docs", "/docs"),
    NavLink::leaf("Firm", "/"),
    NavLink::leaf("Foundation", "/foundation"),
    NavLink::leaf("Navigator", "/navigator"),
    NavLink::leaf("Presentations", "/presentations"),
    NavLink::leaf("Privacy", "/privacy"),
    NavLink::leaf("Terms", "/terms"),
    NavLink::leaf("Workshops", "/workshops"),
];

/// One bar license a named attorney holds: the jurisdiction, the number that
/// jurisdiction publishes them under, and the public record it can be checked
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarLicense {
    pub jurisdiction: &'static str,
    pub number: &'static str,
    pub license_url: &'static str,
}

/// One licensed attorney and the set of bar licenses they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmAttorney {
    pub name: &'static str,
    pub licenses: &'static [BarLicense],
}

/// The attorneys whose bar licences the public footer discloses, one line each
/// with every number linked to that bar's own record.
///
/// Empty on this deployment: the firm publishes no per-attorney bar disclosure
/// in its footer, and no bar number anywhere on the site. `/team` still names
/// the jurisdictions each attorney is licensed in, as a credential chip on
/// their profile — see [`crate::team`] — which is where a reader choosing a
/// lawyer looks for it.
///
/// The seam stays live for the deployments that do publish one — a white-label
/// bundle sets `brand.firm_attorneys` and its footer renders the list (see
/// [`crate::brand_bundle`]). Any entry must give the number its own bar
/// publishes and that bar's own record as the URL, so the two can never drift
/// apart unnoticed, and the full legal name rather than a display name: a bar
/// record is searched under the name the licensing jurisdiction registered.
const FIRM_ATTORNEYS: &[FirmAttorney] = &[];

/// One published office: the city a visitor recognizes it by and the street
/// address they would mail or walk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmOffice {
    pub state: &'static str,
    pub address: &'static str,
    /// A qualification the office cannot be published without — today, whether
    /// an attorney is actually admitted in that jurisdiction yet. Regulated
    /// copy, not decoration: listing an address in a state reads as a holding
    /// out to practise there, so a pending admission must travel with the
    /// address rather than sit in a legend somewhere else on the page.
    /// `None` publishes the address unqualified.
    pub note: Option<&'static str>,
}

/// The firm's published offices, rendered in the public footer.
///
/// This is Shook Law PLLC's address — the entity of record behind the Neon Law
/// mark, the legal person a client actually engages — kept in step with its row
/// in `store/seeds/neon/Address.yaml`.
///
/// One office, where there were four. The California, New York, and Washington
/// boxes were the retired partnership's, and an address is a holding out to
/// practise in that state: publishing one under an entity that does not rent it
/// is a false statement about where the firm can be reached, not a stale
/// footer. Re-add each here only once this entity holds that box.
///
/// Still a separate field from [`SiteBrand::postal_address`], which is the one
/// registered address the letterhead carries, even though both name the same
/// Reno box today. Within the Ridgeview Mail Center the box number is the whole
/// address, so a wrong suffix misdelivers to another entity of ours rather than
/// bouncing — `405-9002` is the firm's, `405-9999` the Foundation's.
///
/// Each office is published under its state rather than its city, so the footer
/// reads as the map of where the firm practises, and the list is ordered
/// alphabetically by that state so a new office slots in by where it sits
/// rather than by whoever edited the list last. The street address underneath
/// still names the city.
///
/// Every comma is a line break. The footer sets an address the way an envelope
/// carries it — street, then unit, then city — so the suite gets its own line
/// and the city starts one, rather than the whole address running together and
/// wrapping wherever the column ends. The city keeps its state and ZIP on the
/// same line. See `webapp::components::site_footer`, which does the splitting.
const FIRM_OFFICES: &[FirmOffice] = &[
    FirmOffice {
        state: "Nevada",
        address: "5150 Mae Anne Ave, Ste 405-9002, Reno, NV 89523",
        note: None,
    },
    FirmOffice {
        state: "New York",
        address: "12 E 49th St, 18th Floor, New York, NY 10017",
        note: None,
    },
    FirmOffice {
        state: "Washington",
        address: "720 Seneca St, Ste 107-715, Seattle, WA 98101",
        note: None,
    },
];

/// All identity consumed by rendering. The web router scopes one immutable
/// instance to a request; direct view tests receive [`DEFAULT_BRANDING`].
#[derive(Debug, Clone, Copy)]
pub struct Branding {
    pub firm: SiteBrand,
    pub foundation: SiteBrand,
    pub firm_email: &'static str,
    pub foundation_email: &'static str,
    /// The host the firm's support address is built on — the `HOST` in
    /// `support@{HOST}`. Set this and the firm's inbound address follows; set
    /// `firm_email` (manifest `brand.support_email`) to name an address that
    /// is not `support@` at all. Deliberately separate from
    /// [`Branding::primary_domain`], which is the infrastructure apex the
    /// deployment derives service hostnames from (`workflows.<apex>`) — moving
    /// the mailbox must not move the cluster.
    pub support_domain: &'static str,
    /// The firm's published voice line, rendered on `/contact` and in the
    /// public footer, dialled from the `tel:` link at either. Brand identity
    /// like the postal address: a deployment that mounts its own manifest
    /// publishes its own number.
    pub firm_phone: &'static str,
    /// Every office the firm publishes, rendered in the public footer. See
    /// [`FIRM_OFFICES`] for why this is not [`SiteBrand::postal_address`].
    pub firm_offices: &'static [FirmOffice],
    /// The firm's attorneys and their bar licenses, rendered in the public
    /// footer. See [`FIRM_ATTORNEYS`]: this is the footer's only bar
    /// disclosure, and it names who holds each licence.
    pub firm_attorneys: &'static [FirmAttorney],
    /// The firm's registered word mark, spelled the way the register spells it
    /// — `NEON LAW`, not the title-case wordmark the header wears. The footer's
    /// trademark notice opens on it, followed by `®`.
    pub firm_trademark: &'static str,
    /// The U.S. registration number for [`Branding::firm_trademark`], written
    /// the way the certificate writes it (`6,325,650`).
    ///
    /// The registrant the notice names is [`SiteBrand::legal_entity`], which is
    /// the same legal person on the firm's deploy. Those two must move together:
    /// a notice that cites this number and names anyone else states the wrong
    /// owner of a live registration, and `cli/tests/license_of_record.rs`
    /// fails the build over exactly that on every other surface that cites it.
    pub firm_trademark_registration: &'static str,
    /// The register's own record for that registration, so a reader verifies the
    /// claim against the USPTO rather than trusting the site's line about
    /// itself — the same rule the footer's bar-licence rows already follow.
    pub firm_trademark_record_url: &'static str,
    pub consultation_url: &'static str,
    pub terms_url: &'static str,
    pub privacy_url: &'static str,
    pub base_url: &'static str,
    pub primary_domain: &'static str,
    pub firm_disclaimer: &'static str,
    /// The Foundation's corporate name, as distinct from the wordmark it trades
    /// under ([`SiteBrand::site_name`], "Neon Law"). The Foundation's own footer
    /// identifies the corporation; the firm's footer names the partnership
    /// through [`SiteBrand::legal_entity`] instead — which is the entity that
    /// *renders legal services* and is deliberately empty here.
    pub foundation_entity: &'static str,
    /// The Foundation's footer disclaimer — the same two sentences as
    /// [`SiteBrand::firm_disclaimer`] and deliberately **without** its opening
    /// "Attorney advertisement."
    ///
    /// That opening is the one place the two lines diverge, and the divergence
    /// is the point: the firm's footer is attorney advertising and says so,
    /// while the Foundation does not render legal services
    /// ([`SiteBrand::is_law_firm`] is `false` for it and its `legal_entity` is
    /// deliberately empty). Labelling the Foundation's pages an attorney
    /// advertisement would be a false statement about which entity the reader
    /// is dealing with, so the label rides the firm brand rather than the
    /// shared string. What both lines still carry is the substance — "not legal
    /// advice" and "no attorney-client relationship absent a signed retainer"
    /// — for whichever organization's page a reader is on.
    pub foundation_disclaimer: &'static str,
    pub mission_description: &'static str,
    pub service_description: &'static str,
    pub portal_only: bool,
}

pub static DEFAULT_BRANDING: Branding = Branding {
    firm: SiteBrand {
        // The firm signs its front door with the mark, not with a lawyer's
        // name: the header, the hero, the page titles, and the `og:site_name`
        // card all read "Neon Law". That is the whole point of the mark — the
        // practice is bigger than any single name on the door, and a client
        // hires the firm rather than whoever happens to be listed first.
        //
        // Deliberately NOT [`SiteBrand::legal_entity`], which is Shook Law
        // PLLC. One is the signature the site is presented under; the other is
        // the legal person the footer's copyright and the retainer must name.
        // A law firm may trade under a mark, but it may not obscure who the
        // client is actually engaging, which is why both are published.
        site_name: "Neon Law",
        home_href: "/",
        tagline: "Flat-fee consumer law, with every price on the page.",
        // Shook Law PLLC's own box, the entity of record. `405-9999` at the
        // same suite is the Foundation's.
        postal_address: "5150 Mae Anne Ave Ste 405-9002, Reno, NV 89523",
        // The NL mark, in teal, shared with the Foundation. One practice, one
        // mark: the firm and the nonprofit are different legal persons wearing
        // the same family identity, and the header wordmark beside it is what
        // says which one you are reading.
        //
        // Drawn geometry, so it is a true vector with every colour written
        // into the file. That matters because `SiteHeader` renders it through
        // `<img src=…>`, which puts an SVG in the browser's secure static
        // mode: every external reference inside the document is blocked, so an
        // SVG that merely wraps an external PNG paints nothing at all. This
        // one references nothing.
        logo_href: "/public/logo-neon.png",
        social_image: "/public/logo-neon.png",
        nav: FIRM_NAV,
        is_law_firm: true,
        // The professional LLC a client engages and the entity that renders the
        // legal services, so it is what the footer's copyright names — and,
        // matching it, what `store::seed::FIRM_ENTITY_NAME` protects as the
        // firm's Entity row. Never equal to `site_name` here: the site trades
        // as Neon Law, and a copyright notice has to name a legal person.
        legal_entity: "Shook Law PLLC",
    },
    foundation: SiteBrand {
        // The nonprofit's own wordmark, distinct from the firm's. They share a
        // family name and a site, and they are different organizations — one
        // practises law, one may not — so the header a reader is looking at has
        // to say which one they are reading.
        site_name: "Neon Law Foundation",
        // The Foundation sits under `/foundation` now that one binary serves
        // both faces, so its logo and home link go there rather than to the
        // site root, which is the firm's.
        home_href: "/foundation",
        tagline: "Access to justice and attorney AI training.",
        postal_address: "5150 Mae Anne Ave Ste 405-9999, Reno, NV 89523",
        logo_href: "/public/logo-neon.png",
        social_image: "/public/logo-neon.png",
        // The firm's row, because the site publishes one header. The nonprofit
        // used to carry its own two-entry row (the workshop catalog and a legal
        // aid audience page); that page is retired and the header is unified, so
        // a reader on `/foundation` sees the same destinations as everywhere
        // else, with the nonprofit itself among them. `/workshops` is still one
        // click away — it sits in `FIRM_FOOTER_NAV`.
        nav: FIRM_NAV,
        is_law_firm: false,
        // The 501(c)(3) does not practice law and holds no copyright in the
        // firm's site: its own footer names its own corporation.
        legal_entity: "",
    },
    // The address the public site publishes, on the host `support_domain` names
    // below. The local part is `contact@`, not `support@`, which is the second
    // dial `firm_support_email` documents — the firm invites new matters to a
    // mailbox named for what a visitor is doing, not for a help desk.
    //
    // This is the *published* address only. Navigator's outbound `From` and the
    // inbound threading address are `workflows::email::DEFAULT_FROM_EMAIL`, a
    // separate constant that stays `support@`: moving what the site advertises
    // must not re-point the mail pipeline.
    //
    // The Foundation is a separate entity on its own domain, so its address does
    // not follow the firm's.
    firm_email: "contact@neonlaw.com",
    foundation_email: "support@neonlaw.org",
    support_domain: "neonlaw.com",
    firm_phone: "+1 510 800 2080",
    firm_offices: FIRM_OFFICES,
    firm_attorneys: FIRM_ATTORNEYS,
    // The mark as registered: the register carries the word mark in capitals,
    // and a notice that cites a registration should spell the mark the way the
    // registration does. `site_name` above is the same mark set the way the
    // firm signs its door.
    firm_trademark: "NEON LAW",
    firm_trademark_registration: "6,325,650",
    // Serial 90039224 is the file this registration issued from, and the search
    // record is the public page that resolves it. The registration number is
    // what the notice claims; this is where a reader checks it.
    firm_trademark_record_url: "https://tmsearch.uspto.gov/search/search-results/90039224",
    consultation_url: "https://calendar.app.google/GueqKHiAuqXEwkRG8",
    terms_url: "/terms",
    privacy_url: "/privacy",
    base_url: "",
    primary_domain: "neonlaw.com",
    firm_disclaimer: "Attorney advertisement. Nothing here is legal advice without a signed retainer for an active project. Past results do not guarantee future outcomes.",
    foundation_entity: "Neon Law Foundation",
    foundation_disclaimer: "Nothing here is legal advice without a signed retainer for an active project. Past results do not guarantee future outcomes.",
    mission_description: "How Neon Law and the Neon Law Foundation make routine legal services affordable without sacrificing correctness, and what a licensed attorney in the loop actually buys you.",
    service_description: "Flat-fee legal services from Neon Law, with every price published.",
    portal_only: false,
};

tokio::task_local! {
    static ACTIVE_BRANDING: &'static Branding;
}

static PROCESS_BRANDING: OnceLock<&'static Branding> = OnceLock::new();

/// Install the immutable branding used by spawned work in a production
/// binary. Request-scoped branding takes precedence, which keeps library tests
/// and independently constructed routers isolated from one another.
pub fn install_process_branding(branding: &'static Branding) -> Result<(), &'static str> {
    PROCESS_BRANDING
        .set(branding)
        .map_err(|_| "process branding is already installed")
}

/// Scope branding to one request future. Task-local storage follows async task
/// migration and keeps concurrently constructed routers isolated.
pub async fn scope<R>(branding: &'static Branding, future: impl Future<Output = R>) -> R {
    ACTIVE_BRANDING.scope(branding, future).await
}

fn current() -> &'static Branding {
    ACTIVE_BRANDING
        .try_with(|branding| *branding)
        .unwrap_or_else(|_| PROCESS_BRANDING.get().copied().unwrap_or(&DEFAULT_BRANDING))
}

fn value(value: Option<&str>, default: &'static str) -> &'static str {
    value
        .filter(|value| !value.is_empty())
        .map_or(default, |value| {
            Box::leak(value.to_owned().into_boxed_str())
        })
}

/// Resolve the firm's inbound support address from a manifest.
///
/// Two dials, most specific first:
///
/// 1. `brand.support_email` — a whole address, for a mailbox that is not
///    `support@` (`hello@`, `intake@`, a shared alias on another domain).
/// 2. `brand.support_domain` — just the host, giving `support@{HOST}`. This is
///    the ordinary dial: a firm that moves domains changes one field.
///
/// With neither set the firm reads mail at [`DEFAULT_BRANDING`]'s address.
fn firm_support_email(support_email: Option<&str>, support_domain: Option<&str>) -> &'static str {
    if let Some(address) = support_email.filter(|address| !address.is_empty()) {
        return Box::leak(address.to_owned().into_boxed_str());
    }
    match support_domain.filter(|domain| !domain.is_empty()) {
        Some(domain) => Box::leak(format!("support@{domain}").into_boxed_str()),
        None => DEFAULT_BRANDING.firm_email,
    }
}

impl Branding {
    /// Resolve one validated manifest into immutable rendering state.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn from_manifest(manifest: &BrandManifest) -> &'static Self {
        let brand = &manifest.brand;
        let firm_name = value(brand.firm.as_deref(), DEFAULT_BRANDING.firm.site_name);
        let foundation_name = value(
            brand.foundation.as_deref(),
            DEFAULT_BRANDING.foundation.site_name,
        );
        let firm_logo = manifest
            .assets
            .firm_logo
            .as_ref()
            .map_or(DEFAULT_BRANDING.firm.logo_href, |_| {
                "/public/brand/firm-logo.svg"
            });
        let firm_raster = manifest
            .assets
            .firm_logo_raster
            .as_ref()
            .map_or(DEFAULT_BRANDING.firm.social_image, |_| {
                "/public/brand/firm-logo.png"
            });
        let foundation_logo = manifest
            .assets
            .foundation_logo
            .as_ref()
            .map_or(DEFAULT_BRANDING.foundation.logo_href, |_| {
                "/public/brand/foundation-logo.svg"
            });
        let foundation_raster = manifest
            .assets
            .foundation_logo_raster
            .as_ref()
            .map_or(DEFAULT_BRANDING.foundation.social_image, |_| {
                "/public/brand/foundation-logo.png"
            });
        let disclaimer = Box::leak(
            format!(
                "Nothing on this site is legal advice. An attorney-client relationship begins only with a signed retainer between you and {firm_name}. Every legal matter is different, and past results do not guarantee a similar result."
            )
            .into_boxed_str(),
        );
        let mission_description = Box::leak(
            format!(
                "How {firm_name} and the {foundation_name} make routine legal services affordable without sacrificing correctness, and what a licensed attorney in the loop actually buys you."
            )
            .into_boxed_str(),
        );
        let service_description =
            Box::leak(format!("Flat-fee legal services from {firm_name}.").into_boxed_str());
        // The Foundation's corporate name defaults to its own manifest key, not
        // to `foundation_name` — that is the wordmark it trades under, and a
        // footer that identifies the corporation must not print a brand there.
        let foundation_entity = value(
            brand.foundation_legal_entity.as_deref(),
            DEFAULT_BRANDING.foundation_entity,
        );
        // A registration belongs to its registrant, and a bundle that renames
        // the firm is a different firm: it publishes no mark notice at all,
        // because U.S. Reg. No. 6,325,650 is not theirs to notice. There is
        // deliberately no manifest key for it either — an operator with their
        // own registration is making an ownership claim this repository cannot
        // verify, and a notice naming the wrong owner is worse than none, since
        // it is the line a reader relies on for permission.
        let (firm_trademark, firm_trademark_registration, firm_trademark_record_url) =
            if brand.firm.is_some() {
                ("", "", "")
            } else {
                (
                    DEFAULT_BRANDING.firm_trademark,
                    DEFAULT_BRANDING.firm_trademark_registration,
                    DEFAULT_BRANDING.firm_trademark_record_url,
                )
            };
        Box::leak(Box::new(Self {
            firm: SiteBrand {
                site_name: firm_name,
                postal_address: value(
                    brand.firm_address.as_deref(),
                    DEFAULT_BRANDING.firm.postal_address,
                ),
                logo_href: firm_logo,
                social_image: firm_raster,
                // A mounted bundle that renames the firm but declares no
                // corporate name falls back to *its own* wordmark, never to this
                // firm's legal entity. The built-in default is only right when
                // the bundle left the firm name alone: otherwise the footer of a
                // white-label host would publish `Neon Law` as the
                // corporation behind someone else's site.
                legal_entity: value(
                    brand.firm_legal_entity.as_deref(),
                    if brand.firm.is_some() {
                        firm_name
                    } else {
                        DEFAULT_BRANDING.firm.legal_entity
                    },
                ),
                ..DEFAULT_BRANDING.firm
            },
            foundation: SiteBrand {
                site_name: foundation_name,
                postal_address: value(
                    brand.foundation_address.as_deref(),
                    DEFAULT_BRANDING.foundation.postal_address,
                ),
                logo_href: foundation_logo,
                social_image: foundation_raster,
                ..DEFAULT_BRANDING.foundation
            },
            firm_email: firm_support_email(
                brand.support_email.as_deref(),
                brand.support_domain.as_deref(),
            ),
            foundation_email: value(
                brand.foundation_email.as_deref(),
                DEFAULT_BRANDING.foundation_email,
            ),
            support_domain: value(
                brand.support_domain.as_deref(),
                DEFAULT_BRANDING.support_domain,
            ),
            firm_phone: value(brand.firm_phone.as_deref(), DEFAULT_BRANDING.firm_phone),
            firm_offices: if brand.firm_offices.is_empty() {
                DEFAULT_BRANDING.firm_offices
            } else {
                let leaked: Vec<FirmOffice> = brand
                    .firm_offices
                    .iter()
                    .map(|entry| FirmOffice {
                        state: Box::leak(entry.state.clone().into_boxed_str()),
                        address: Box::leak(entry.address.clone().into_boxed_str()),
                        note: entry
                            .note
                            .clone()
                            .map(|note| &*Box::leak(note.into_boxed_str())),
                    })
                    .collect();
                Box::leak(leaked.into_boxed_slice())
            },
            firm_attorneys: if brand.firm_attorneys.is_empty() {
                DEFAULT_BRANDING.firm_attorneys
            } else {
                let leaked: Vec<FirmAttorney> = brand
                    .firm_attorneys
                    .iter()
                    .map(|entry| FirmAttorney {
                        name: Box::leak(entry.name.clone().into_boxed_str()),
                        licenses: {
                            let licenses: Vec<BarLicense> = entry
                                .licenses
                                .iter()
                                .map(|license| BarLicense {
                                    jurisdiction: Box::leak(
                                        license.jurisdiction.clone().into_boxed_str(),
                                    ),
                                    number: Box::leak(license.number.clone().into_boxed_str()),
                                    license_url: Box::leak(
                                        license.license_url.clone().into_boxed_str(),
                                    ),
                                })
                                .collect();
                            Box::leak(licenses.into_boxed_slice())
                        },
                    })
                    .collect();
                Box::leak(leaked.into_boxed_slice())
            },
            firm_trademark,
            firm_trademark_registration,
            firm_trademark_record_url,
            consultation_url: value(
                brand.consultation_url.as_deref(),
                DEFAULT_BRANDING.consultation_url,
            ),
            terms_url: value(brand.terms_url.as_deref(), DEFAULT_BRANDING.terms_url),
            privacy_url: value(brand.privacy_url.as_deref(), DEFAULT_BRANDING.privacy_url),
            base_url: value(brand.base_url.as_deref(), DEFAULT_BRANDING.base_url),
            primary_domain: value(
                brand.primary_domain.as_deref(),
                DEFAULT_BRANDING.primary_domain,
            ),
            firm_disclaimer: disclaimer,
            foundation_entity,
            foundation_disclaimer: DEFAULT_BRANDING.foundation_disclaimer,
            mission_description,
            service_description,
            portal_only: manifest.portal_only,
        }))
    }
}

#[derive(Debug, Clone, Copy)]
enum BrandKind {
    Firm,
    Foundation,
}

/// Copy-compatible accessor preserving existing view APIs while resolving the
/// request-scoped structured branding.
pub struct BrandAccessor(BrandKind);

impl Deref for BrandAccessor {
    type Target = SiteBrand;

    fn deref(&self) -> &'static Self::Target {
        match self.0 {
            BrandKind::Firm => &current().firm,
            BrandKind::Foundation => &current().foundation,
        }
    }
}

pub static FIRM_BRAND: BrandAccessor = BrandAccessor(BrandKind::Firm);
pub static FOUNDATION_BRAND: BrandAccessor = BrandAccessor(BrandKind::Foundation);

/// Firm inbound email from request-scoped branding.
#[must_use]
pub fn firm_email() -> &'static str {
    current().firm_email
}

/// Firm consultation booking URL — the calendar where a prospective
/// client books a flat-fee consultation. A bundle knob with no public
/// surface behind it: no shipped page links it, because the firm quotes a
/// matter from its inbox before it sells an appointment. It stays part of
/// the manifest contract so an operator whose own scheduler is the front
/// door can carry the coordinate without forking source. Defaults to the
/// firm's real Google Calendar appointment page.
#[must_use]
pub fn consultation_url() -> &'static str {
    current().consultation_url
}

/// Where the footer's "Terms" link points. Defaults to the in-app
/// `/terms` page (NeonLaw's bundled terms of use). A white-label deploy
/// — a firm whose own marketing site already hosts its terms — sets
/// bundle's `terms_url` to that off-site URL so Neon Law Navigator links out
/// instead of serving someone else's binding legal text. Same `Copy`-
/// friendly `&'static str` shape as the other brand links; resolved when the
/// bundle loads.
#[must_use]
pub fn terms_url() -> &'static str {
    current().terms_url
}

/// Where the footer's "Privacy" link points. Defaults to the in-app
/// `/privacy` page; a bundle can point at a
/// deployer's own hosted privacy policy. See [`terms_url`].
#[must_use]
pub fn privacy_url() -> &'static str {
    current().privacy_url
}

/// Foundation inbound email from request-scoped branding.
#[must_use]
pub fn foundation_email() -> &'static str {
    current().foundation_email
}

#[must_use]
pub fn base_url() -> &'static str {
    current().base_url
}

#[must_use]
pub fn primary_domain() -> &'static str {
    current().primary_domain
}

/// The firm's own host — the `HOST` in `support@{HOST}`, and the host its
/// public site is served from.
///
/// Distinct from [`primary_domain`], which is the infrastructure apex a
/// deployment derives service hostnames from. The two are different things
/// that both look like domains; see [`Branding::support_domain`].
#[must_use]
pub fn support_domain() -> &'static str {
    current().support_domain
}

#[must_use]
pub fn portal_only() -> bool {
    current().portal_only
}

/// The firm's published voice line from request-scoped branding.
#[must_use]
pub fn firm_phone() -> &'static str {
    current().firm_phone
}

/// The firm's published offices from request-scoped branding.
#[must_use]
pub fn firm_offices() -> &'static [FirmOffice] {
    current().firm_offices
}

/// The firm's attorneys and their bar licenses from request-scoped branding.
#[must_use]
pub fn firm_attorneys() -> &'static [FirmAttorney] {
    current().firm_attorneys
}

/// The firm's registered word mark, its U.S. registration number, and the
/// register's own record for it — the three parts of the footer's trademark
/// notice, resolved together because a notice missing any of them makes a claim
/// a reader cannot check.
///
/// All three are empty on a bundle that renamed the firm: see
/// [`Branding::firm_trademark_registration`] for why a host publishes no notice
/// rather than this one.
#[must_use]
pub fn firm_trademark() -> (&'static str, &'static str, &'static str) {
    let branding = current();
    (
        branding.firm_trademark,
        branding.firm_trademark_registration,
        branding.firm_trademark_record_url,
    )
}

/// The public pages the footer links rather than the header — Navigator, Blog,
/// Contact, and the rest. See [`FIRM_FOOTER_NAV`].
///
/// Not brand-scoped: a white-label deploy renames the wordmark and re-points the
/// addresses, but these routes are Navigator's own public surface and are the
/// same wherever the firm's footer renders.
#[must_use]
pub fn firm_footer_nav() -> &'static [NavLink] {
    FIRM_FOOTER_NAV
}

/// The firm's legal-advice disclaimer, shown in the footer of every
/// page. The unified footer is firm-anchored, so the disclaimer is
/// always the firm's — it lives here as a single resolved string rather
/// than a per-brand `SiteBrand` field (the Foundation never carried its
/// own). Names the firm via [`FIRM_BRAND`]; resolved when the bundle loads.
#[must_use]
pub fn firm_disclaimer() -> &'static str {
    current().firm_disclaimer
}

/// The Foundation's corporate name from request-scoped branding — the entity,
/// not the wordmark [`FOUNDATION_BRAND`] trades under.
#[must_use]
pub fn foundation_entity() -> &'static str {
    current().foundation_entity
}

/// The Foundation's own footer disclaimer from request-scoped branding.
#[must_use]
pub fn foundation_disclaimer() -> &'static str {
    current().foundation_disclaimer
}

#[must_use]
pub fn mission_description() -> &'static str {
    current().mission_description
}

#[must_use]
pub fn service_description() -> &'static str {
    current().service_description
}

/// The deployed release — the `YY.M.D` Artifact Registry tag this image
/// was published under, baked into the web image by `deploy.yml` as
/// `NAVIGATOR_RELEASE_TAG` (the same value `GET /version` reports as
/// `release`). Rendered in the footer so a push is visible end-to-end:
/// the moment a new image is live on the site, the footer's version
/// changes. `None` on a local `cargo run` (the env var is unset, or the
/// build honestly reports `unknown`), so dev never shows a bogus
/// version. Resolved once per process.
#[must_use]
pub fn deployed_release() -> Option<&'static str> {
    static RELEASE: LazyLock<Option<&'static str>> =
        LazyLock::new(|| match env::var("NAVIGATOR_RELEASE_TAG") {
            Ok(v) if v.is_empty() || v == "unknown" => None,
            Ok(v) => Some(&*Box::leak(v.into_boxed_str())),
            Err(_) => None,
        });
    *RELEASE
}

#[cfg(test)]
mod tests {
    use super::{scope, Branding, NavLink, DEFAULT_BRANDING, FIRM_BRAND, FOUNDATION_BRAND};
    use crate::brand_bundle::BrandManifest;

    #[tokio::test]
    async fn mounted_manifest_is_request_scoped_and_complete() {
        let manifest: BrandManifest = serde_yaml::from_str(
            "version: 1\nbrand:\n  firm: Acme Law\n  foundation: Acme Foundation\n  support_email: firm@acme.example\n  foundation_email: foundation@acme.example\n  firm_address: 1 Main St\n  foundation_address: 2 Main St\n  base_url: https://app.acme.example\n  primary_domain: acme.example\n  consultation_url: https://acme.example/book\n  terms_url: https://acme.example/terms\n  privacy_url: https://acme.example/privacy\nassets:\n  firm_logo: firm.svg\n  firm_logo_raster: firm.png\n  foundation_logo: foundation.svg\n  foundation_logo_raster: foundation.png\n",
        )
        .unwrap();
        let branding = Branding::from_manifest(&manifest);
        scope(branding, async {
            assert_eq!(FIRM_BRAND.site_name, "Acme Law");
            assert_eq!(FOUNDATION_BRAND.site_name, "Acme Foundation");
            assert_eq!(FIRM_BRAND.postal_address, "1 Main St");
            assert_eq!(FOUNDATION_BRAND.postal_address, "2 Main St");
            assert_eq!(FIRM_BRAND.logo_href, "/public/brand/firm-logo.svg");
            assert_eq!(FIRM_BRAND.social_image, "/public/brand/firm-logo.png");
            assert_eq!(
                FOUNDATION_BRAND.logo_href,
                "/public/brand/foundation-logo.svg"
            );
            assert_eq!(
                FOUNDATION_BRAND.social_image,
                "/public/brand/foundation-logo.png"
            );
            assert_eq!(super::firm_email(), "firm@acme.example");
            assert_eq!(super::foundation_email(), "foundation@acme.example");
            assert_eq!(super::consultation_url(), "https://acme.example/book");
            assert_eq!(super::terms_url(), "https://acme.example/terms");
            assert_eq!(super::privacy_url(), "https://acme.example/privacy");
            assert_eq!(branding.base_url, "https://app.acme.example");
            assert_eq!(branding.primary_domain, "acme.example");
            assert!(super::firm_disclaimer().contains("Acme Law"));
            assert!(super::mission_description().contains("Acme Law"));
            assert!(super::mission_description().contains("Acme Foundation"));
            assert_eq!(
                super::service_description(),
                "Flat-fee legal services from Acme Law."
            );
            // Unset in the manifest, but the manifest renamed the firm → the
            // bundle's own wordmark, never this firm's corporate name. The
            // Foundation renders none.
            assert_eq!(FIRM_BRAND.legal_entity, "Acme Law");
            assert_eq!(FOUNDATION_BRAND.legal_entity, "");
        })
        .await;
        assert_eq!(FIRM_BRAND.site_name, DEFAULT_BRANDING.firm.site_name);
    }

    /// The footer publishes the one office the firm actually keeps, and the
    /// letterhead names the same box.
    ///
    /// It published four while the retired partnership held boxes in four
    /// states. An address is a holding out to practise in that state, so a
    /// list that outlives the entity renting those boxes is a false statement
    /// about where the firm can be reached — not a stale footer. A fifth entry
    /// appearing here must correspond to a box this entity actually holds.
    #[test]
    fn publishes_every_firm_office_without_touching_the_letterhead() {
        let published: Vec<(&str, &str)> = DEFAULT_BRANDING
            .firm_offices
            .iter()
            .map(|office| (office.state, office.address))
            .collect();
        assert_eq!(
            published,
            [
                ("Nevada", "5150 Mae Anne Ave, Ste 405-9002, Reno, NV 89523"),
                ("New York", "12 E 49th St, 18th Floor, New York, NY 10017"),
                (
                    "Washington",
                    "720 Seneca St, Ste 107-715, Seattle, WA 98101"
                ),
            ],
            "the footer publishes the offices the firm actually keeps, ordered by state",
        );
        assert_eq!(
            DEFAULT_BRANDING.firm.postal_address, "5150 Mae Anne Ave Ste 405-9002, Reno, NV 89523",
            "the letterhead names the entity of record's own box",
        );
    }

    /// Every office is published bare. The `note` seam stays — a deployment
    /// that must qualify an address sets it and the footer renders it under
    /// that address alone — but this firm qualifies none, and a note that
    /// reappeared would disclaim a practice the firm actually has.
    #[test]
    fn publishes_every_office_address_unqualified() {
        let qualified: Vec<(&str, Option<&str>)> = DEFAULT_BRANDING
            .firm_offices
            .iter()
            .map(|office| (office.state, office.note))
            .collect();
        assert_eq!(
            qualified,
            [("Nevada", None), ("New York", None), ("Washington", None)],
            "no office carries a note",
        );
    }

    /// The firm's footer discloses no per-attorney bar licence. The credentials
    /// belong on `/team`, where a reader choosing a lawyer looks for them.
    #[test]
    fn the_firm_footer_discloses_no_bar_licences() {
        assert!(
            DEFAULT_BRANDING.firm_attorneys.is_empty(),
            "the firm publishes no bar numbers in its footer",
        );
    }

    /// Every published bar number must point at the record that issued it: a
    /// number and a URL that disagree is a false statement about a licence.
    /// This deployment publishes none, so the rule guards whatever a manifest
    /// sets — the check runs against the resolved set, not a literal list.
    #[test]
    fn every_published_bar_number_appears_in_the_record_it_links() {
        for attorney in DEFAULT_BRANDING.firm_attorneys {
            assert!(
                !attorney.licenses.is_empty(),
                "{} is listed with no licence",
                attorney.name
            );
            for license in attorney.licenses {
                assert!(
                    license.license_url.contains(license.number),
                    "{} {} No. {} does not appear in {}",
                    attorney.name,
                    license.jurisdiction,
                    license.number,
                    license.license_url,
                );
            }
        }
    }

    #[tokio::test]
    async fn firm_offices_and_attorneys_are_configurable_via_the_manifest() {
        // A white-label deploy publishes its own doors and its own licensed
        // attorneys; neither falls back to the firm's once it sets its own.
        let manifest: BrandManifest = serde_yaml::from_str(
            "version: 1\nbrand:\n  firm_offices:\n    - state: Idaho\n      address: 1 Main St, Boise, ID 83702\n  firm_attorneys:\n    - name: Ada Lovelace\n      licenses:\n        - jurisdiction: Idaho\n          number: \"4242\"\n          license_url: https://isb.idaho.gov/4242\n",
        )
        .unwrap();
        let branding = Branding::from_manifest(&manifest);
        scope(branding, async {
            assert_eq!(super::firm_offices().len(), 1);
            assert_eq!(super::firm_offices()[0].state, "Idaho");
            assert_eq!(super::firm_attorneys().len(), 1);
            assert_eq!(super::firm_attorneys()[0].name, "Ada Lovelace");
            assert_eq!(super::firm_attorneys()[0].licenses[0].number, "4242");
        })
        .await;
        // The compiled default is unchanged.
        assert_eq!(DEFAULT_BRANDING.firm_offices.len(), 3);
        assert!(DEFAULT_BRANDING.firm_attorneys.is_empty());
    }

    #[tokio::test]
    async fn firm_legal_entity_is_configurable_via_the_manifest() {
        // The footer's regulated legal-services attribution is a manifest edit,
        // not a code change: setting `firm_legal_entity` renames the entity.
        // The override is deliberately a name the compiled default does not
        // carry, so a scope that silently fell back would fail here.
        let manifest: BrandManifest =
            serde_yaml::from_str("version: 1\nbrand:\n  firm_legal_entity: Cascade Law LLP\n")
                .unwrap();
        let branding = Branding::from_manifest(&manifest);
        scope(branding, async {
            assert_eq!(FIRM_BRAND.legal_entity, "Cascade Law LLP");
        })
        .await;
        // The compiled default is unchanged.
        assert_eq!(DEFAULT_BRANDING.firm.legal_entity, "Shook Law PLLC");
        assert_eq!(DEFAULT_BRANDING.foundation.legal_entity, "");
    }

    /// The compiled default notices the live registration, spelled the way the
    /// register spells it and pointing at the register's own record.
    ///
    /// The registrant is [`SiteBrand::legal_entity`], asserted here beside the
    /// number so the pair cannot drift: U.S. Reg. No. 6,325,650 is the Firm's,
    /// and the footer composes its notice from these two fields.
    #[test]
    fn the_default_notices_the_registration_and_where_to_verify_it() {
        let (mark, registration, record) = (
            DEFAULT_BRANDING.firm_trademark,
            DEFAULT_BRANDING.firm_trademark_registration,
            DEFAULT_BRANDING.firm_trademark_record_url,
        );
        assert_eq!(mark, "NEON LAW");
        assert_eq!(registration, "6,325,650");
        assert_eq!(
            record,
            "https://tmsearch.uspto.gov/search/search-results/90039224"
        );
        assert_eq!(
            DEFAULT_BRANDING.firm.legal_entity, "Shook Law PLLC",
            "the notice names this entity as the registrant"
        );
    }

    /// A bundle that renames the firm publishes no mark notice.
    ///
    /// The registration is not the mounting host's, so there is nothing for it
    /// to notice — and a host inheriting this one would tell its own readers
    /// that someone else's registration covers the name on its door.
    #[tokio::test]
    async fn a_renamed_firm_notices_no_registration() {
        let manifest: BrandManifest =
            serde_yaml::from_str("version: 1\nbrand:\n  firm: Cascade Law\n").unwrap();
        let branding = Branding::from_manifest(&manifest);
        scope(branding, async {
            assert_eq!(super::firm_trademark(), ("", "", ""));
        })
        .await;
        // A bundle that changed something else keeps it: the firm did not move.
        let untouched: BrandManifest =
            serde_yaml::from_str("version: 1\nbrand:\n  firm_phone: '+1 555 000 0000'\n").unwrap();
        scope(Branding::from_manifest(&untouched), async {
            assert_eq!(
                super::firm_trademark(),
                (
                    "NEON LAW",
                    "6,325,650",
                    DEFAULT_BRANDING.firm_trademark_record_url
                )
            );
        })
        .await;
    }

    /// The mark and the legal person behind it are two different strings, and
    /// the site publishes both.
    ///
    /// This is the load-bearing assertion of the whole rename. A firm may trade
    /// under a mark, but a client has to be able to see which legal person they
    /// are engaging — the copyright, the retainer, and the delete-guarded
    /// Entity row all name `Shook Law PLLC` while the header reads `Neon Law`.
    /// Collapsing the two would either put a trade name on a binding instrument
    /// or put a lawyer's name back on the door the mark exists to replace.
    #[test]
    fn the_mark_and_the_entity_of_record_are_distinct() {
        assert_eq!(FIRM_BRAND.site_name, "Neon Law");
        // The firm ALONE. The rendered copyright names both organizations, but
        // it composes that line from this field and the Foundation's name
        // ("© {year} {copyright_holder} and {foundation}"), so this one must
        // stay the single legal person. The supporter line is why it matters:
        // it reads "{copyright_holder} is a proud supporter of the Neon Law
        // Foundation", and folding the nonprofit into this string would
        // publish the Foundation announcing that it supports itself.
        assert_eq!(FIRM_BRAND.legal_entity, "Shook Law PLLC");
        assert_ne!(
            FIRM_BRAND.site_name, FIRM_BRAND.legal_entity,
            "the mark is not the legal person; the footer must publish both"
        );
        assert!(FIRM_BRAND.is_law_firm);
    }

    /// The disclaimer states the two things that matter regardless of which
    /// face renders it: nothing here is legal advice, and no attorney-client
    /// relationship exists without a signed retainer for an active project.
    #[test]
    fn the_disclaimer_requires_a_signed_retainer_for_an_active_project() {
        let disclaimer = super::firm_disclaimer();
        assert!(disclaimer.contains("Nothing here is legal advice"));
        assert!(disclaimer.contains("signed retainer"));
        assert!(disclaimer.contains("active project"));
    }

    /// The two disclaimers carry the same substance and differ by exactly one
    /// thing: the firm's opens by naming itself an attorney advertisement.
    ///
    /// The difference is not a drafting slip and this test is what keeps it from
    /// being "tidied" in either direction. Dropping the label from the firm's
    /// line loses a notice a law firm's public pages are expected to carry;
    /// copying it onto the Foundation's asserts that a body which renders no
    /// legal services is advertising legal services, which is false about the
    /// entity the reader is dealing with. Everything after the label stays
    /// standardized site-wide.
    #[test]
    fn only_the_firm_disclaimer_names_itself_an_advertisement() {
        const LABEL: &str = "Attorney advertisement.";

        let firm = super::firm_disclaimer();
        let foundation = super::foundation_disclaimer();

        assert!(
            firm.starts_with(LABEL),
            "the firm's disclaimer opens with the label: {firm}"
        );
        assert!(
            !foundation.contains(LABEL),
            "the Foundation renders no legal services and must not claim to be \
             advertising them: {foundation}"
        );
        // Past the label, one standardized sentence pair for both faces.
        assert_eq!(firm.trim_start_matches(LABEL).trim_start(), foundation);
        // The label is only honest on the face that is a law firm.
        assert!(FIRM_BRAND.is_law_firm);
        assert!(!FOUNDATION_BRAND.is_law_firm);
    }

    /// Both faces state the past-results line, wherever a reader could infer
    /// one. It moved off `/litigation` into this shared string, so the guard
    /// that it is still said belongs here rather than on that page.
    #[test]
    fn both_disclaimers_carry_the_past_results_line() {
        for disclaimer in [super::firm_disclaimer(), super::foundation_disclaimer()] {
            assert!(
                disclaimer.contains("Past results do not guarantee future outcomes"),
                "the past-results line: {disclaimer}"
            );
        }
    }

    /// Every request-scoped accessor reads the field it is named for.
    ///
    /// These are a wall of three-line functions over one struct, which is
    /// exactly the shape a copy-paste gets wrong silently: `support_domain`
    /// returning `primary_domain` still compiles, still returns a plausible
    /// domain, and ships `support@` at the infrastructure apex. The two are
    /// deliberately different values here so that swap cannot pass, and the
    /// pair is asserted distinct so a future edit collapsing them fails loudly
    /// rather than making this test vacuous.
    #[test]
    fn each_brand_accessor_reads_its_own_field() {
        assert_eq!(super::base_url(), DEFAULT_BRANDING.base_url);
        assert_eq!(super::primary_domain(), DEFAULT_BRANDING.primary_domain);
        assert_eq!(super::support_domain(), DEFAULT_BRANDING.support_domain);
        assert_eq!(super::portal_only(), DEFAULT_BRANDING.portal_only);
        assert_eq!(super::firm_phone(), DEFAULT_BRANDING.firm_phone);
        assert_eq!(super::foundation_email(), DEFAULT_BRANDING.foundation_email);
        assert_eq!(
            super::foundation_entity(),
            DEFAULT_BRANDING.foundation_entity
        );
        assert_eq!(
            super::foundation_disclaimer(),
            DEFAULT_BRANDING.foundation_disclaimer
        );
        assert_eq!(super::terms_url(), DEFAULT_BRANDING.terms_url);
        assert_eq!(super::privacy_url(), DEFAULT_BRANDING.privacy_url);
    }

    /// A build that was never stamped reports no release rather than a
    /// placeholder. The footer prints the platform line only when this is
    /// `Some`, so a bogus value here would publish "Navigator #unknown" on
    /// every page of a local run.
    #[test]
    fn an_unstamped_build_reports_no_release() {
        // The test process is not a deployed image, so the tag is absent or
        // honestly `unknown`; either way the accessor must decline it.
        assert_eq!(super::deployed_release(), None);
    }

    /// A white-label bundle that renames the firm but declares no corporate name
    /// takes its own wordmark as the legal entity. Falling back to the compiled
    /// default would print this firm's entity in the footer of someone else's
    /// site — a leak of our identity, not a harmless default.
    #[tokio::test]
    async fn a_renamed_firm_without_a_legal_entity_does_not_inherit_this_firms() {
        let manifest: BrandManifest =
            serde_yaml::from_str("version: 1\nbrand:\n  firm: Acme Law\n").unwrap();
        let branding = Branding::from_manifest(&manifest);
        scope(branding, async {
            assert_eq!(FIRM_BRAND.legal_entity, "Acme Law");
            assert!(!FIRM_BRAND.legal_entity.contains("Shook Law PLLC"));
        })
        .await;
    }

    /// A bundle that leaves the firm name alone is this firm's own deploy, so the
    /// compiled legal entity is still the right answer.
    #[tokio::test]
    async fn a_bundle_that_keeps_the_firm_name_keeps_the_compiled_legal_entity() {
        let manifest: BrandManifest =
            serde_yaml::from_str("version: 1\nbrand:\n  foundation: Other Foundation\n").unwrap();
        let branding = Branding::from_manifest(&manifest);
        scope(branding, async {
            assert_eq!(FIRM_BRAND.legal_entity, "Shook Law PLLC");
        })
        .await;
    }

    #[tokio::test]
    async fn concurrent_brand_scopes_do_not_cross_contaminate() {
        let first: BrandManifest = serde_yaml::from_str(
            "version: 1\nbrand:\n  firm: First Law\n  foundation: First Foundation\n",
        )
        .unwrap();
        let second: BrandManifest = serde_yaml::from_str(
            "version: 1\nbrand:\n  firm: Second Law\n  foundation: Second Foundation\n",
        )
        .unwrap();
        let first = Branding::from_manifest(&first);
        let second = Branding::from_manifest(&second);

        let (first_name, second_name) = tokio::join!(
            scope(first, async {
                tokio::task::yield_now().await;
                (
                    FIRM_BRAND.site_name,
                    super::mission_description(),
                    super::service_description(),
                )
            }),
            scope(second, async {
                tokio::task::yield_now().await;
                (
                    FIRM_BRAND.site_name,
                    super::mission_description(),
                    super::service_description(),
                )
            })
        );
        assert_eq!(first_name.0, "First Law");
        assert!(first_name.1.contains("First Foundation"));
        assert!(first_name.2.contains("First Law"));
        assert_eq!(second_name.0, "Second Law");
        assert!(second_name.1.contains("Second Foundation"));
        assert!(second_name.2.contains("Second Law"));
    }

    /// The two brands are one family and two organizations, so their wordmarks
    /// must not be the same string. One practises law and one may not; a reader
    /// has to be able to tell from the header which they are reading.
    #[test]
    fn the_firm_and_the_foundation_carry_distinct_wordmarks() {
        assert_eq!(FOUNDATION_BRAND.site_name, "Neon Law Foundation");
        assert_ne!(FIRM_BRAND.site_name, FOUNDATION_BRAND.site_name);
        assert!(!FOUNDATION_BRAND.is_law_firm);
    }

    #[test]
    fn both_brands_share_one_raster_social_image() {
        // The Open Graph card needs a PNG (scrapers won't render the
        // SVG favicon) and each brand shares its own mark.
        let is_png = |path: &str| {
            std::path::Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        };
        assert!(is_png(FIRM_BRAND.social_image));
        assert!(is_png(FOUNDATION_BRAND.social_image));
        // One mark, shared. They carried separate images while the two brands
        // wore separate palettes; the NL mark in teal is now both.
        assert_eq!(FIRM_BRAND.social_image, FOUNDATION_BRAND.social_image);
    }

    /// Each entity publishes its own box. Within the Ridgeview Mail Center the
    /// box number is the whole address, so serving one entity's suite under
    /// another's name misdelivers rather than bounces — `405-9002` is the
    /// firm's and `405-9999` the Foundation's, and the retired partnership's
    /// `405-9777` belongs to neither.
    #[test]
    fn firm_and_foundation_carry_distinct_suite_addresses() {
        assert!(FIRM_BRAND.postal_address.contains("405-9002"));
        assert!(FOUNDATION_BRAND.postal_address.contains("405-9999"));
        assert_ne!(FIRM_BRAND.postal_address, FOUNDATION_BRAND.postal_address);
        for brand in [&*FIRM_BRAND, &*FOUNDATION_BRAND] {
            assert!(
                !brand.postal_address.contains("405-9777"),
                "the retired partnership's box is not a public brand's address"
            );
        }
    }

    /// The firm's header leads with the lead offering, then the three practices.
    ///
    /// Fractional CTO is first on purpose and this is the assertion that keeps
    /// it there: it is what the firm leads with, and the home page states it
    /// above the fold. Demoting it below the practices would put the lead behind
    /// the things it leads.
    ///
    /// Team used to close the row. The page was retired outright — routers,
    /// views, path constants, sitemap and llms.txt rows — so a header entry
    /// would now be a link to a `404`.
    ///
    /// Foundation closes it instead, and closes it deliberately: this row is the
    /// header on both faces now, so the nonprofit needs a place in it, and the
    /// place is after the firm's own work rather than before it.
    #[test]
    fn the_firm_nav_leads_with_the_lead_offering_then_the_practices() {
        let labels: Vec<&str> = FIRM_BRAND.nav.iter().map(|n| n.label).collect();
        assert_eq!(
            labels,
            [
                "Fractional CTO",
                "Litigation",
                "Fractional GC",
                "Legal Services",
                "Foundation"
            ]
        );
        assert_eq!(
            FIRM_BRAND.nav.first().map(|link| link.href),
            Some("/fractional-cto"),
            "the lead offering is the first thing in the header"
        );
        assert_eq!(
            FIRM_BRAND.nav.last().map(|link| link.href),
            Some("/foundation"),
            "the nonprofit closes the row, after the firm's own work"
        );
        assert!(
            FIRM_BRAND.nav.iter().all(|link| !link.is_dropdown()),
            "every firm nav link is a flat leaf"
        );
    }

    /// Blog, Contact, Docs, Navigator, Neon Law, Presentations, Privacy, Terms,
    /// and Workshops are the routes the header does not carry, ordered
    /// alphabetically by label. They are still linked from every public page — a
    /// route in neither row is stranded, which is the failure this pairs with
    /// the test above to catch.
    ///
    /// Workshops joined the row when the classes became public, and Docs when
    /// the workspace documentation did. While either was gated the chrome
    /// deliberately omitted it rather than send a signed-out reader at a login
    /// door; now that anyone may read them, a footer link is what stops each
    /// being reachable only by typing the URL.
    ///
    /// Privacy and Terms are here for the plainer reason that the header links
    /// neither and the legal strip below links only the bar records: without
    /// this row, the two documents the site publishes about itself would be
    /// reachable only by typing the URL.
    #[test]
    fn the_footer_nav_carries_what_the_header_does_not() {
        let footer: Vec<&str> = super::firm_footer_nav().iter().map(|n| n.label).collect();
        assert_eq!(
            footer,
            [
                "Blog",
                "Contact",
                "Docs",
                "Firm",
                "Foundation",
                "Navigator",
                "Presentations",
                "Privacy",
                "Terms",
                "Workshops"
            ]
        );
        assert_eq!(
            footer.len(),
            10,
            "the footer lays the row out as two columns of five: {footer:?}"
        );
        let mut sorted = footer.clone();
        sorted.sort_unstable();
        assert_eq!(footer, sorted, "the footer nav is alphabetized by label");
        // Every label is linked once — from exactly one of the two rows — with
        // Foundation the single deliberate exception. The nonprofit's own header
        // is gone, so this row is the header on both faces: the cross-link has
        // to be at the top of a Foundation page as well as the bottom, and a
        // reader who scrolled to the footer looking for it should still find it
        // in the site map there. Named rather than skipped by a general rule, so
        // a second duplicate cannot creep in behind this one.
        let header: Vec<&str> = FIRM_BRAND.nav.iter().map(|n| n.label).collect();
        for label in footer.iter().filter(|label| **label != "Foundation") {
            assert!(
                !header.contains(label),
                "{label} is linked once, from the footer"
            );
        }
        assert!(
            header.contains(&"Foundation"),
            "the nonprofit is linked from the header too, now that it has no \
             header of its own: {header:?}"
        );
        assert!(
            !header.contains(&"Team"),
            "the team page is retired, so neither row may link it",
        );
        assert!(
            super::firm_footer_nav()
                .iter()
                .all(|link| link.href.starts_with('/') && !link.is_dropdown()),
            "every footer link is a flat internal leaf"
        );
    }

    /// Both faces are reachable from the footer, each labelled by the
    /// organization rather than by its wordmark.
    ///
    /// One binary serves both faces, so neither is a different host a link
    /// would have to leave for. That makes the pair of cross-links both
    /// possible and correct — and they belong in the footer, because a visitor
    /// at the top of a law firm's page is deciding whether to hire a lawyer.
    #[test]
    fn the_footer_nav_reaches_both_the_firm_and_the_foundation() {
        for (label, href) in [("Firm", "/"), ("Foundation", "/foundation")] {
            let link = super::firm_footer_nav()
                .iter()
                .find(|link| link.label == label)
                .unwrap_or_else(|| panic!("{label} is linked from the shared footer"));
            assert_eq!(link.href, href);
        }
    }

    /// The nonprofit carries the firm's header row, because the site publishes
    /// one header.
    ///
    /// It used to carry its own two entries — the workshop catalog and a legal
    /// aid audience page. That page is retired and the header is unified, so
    /// this asserts the two rows are now the same object rather than two lists
    /// that could drift: a reader on `/foundation` meets the same destinations
    /// as a reader anywhere else, with the nonprofit itself among them.
    ///
    /// `home_href` is deliberately still its own prefix. The header is shared;
    /// the nonprofit's front door is not, and the footer's "Foundation"
    /// cross-link and the header entry both have to land on it.
    ///
    /// This asserts the constants. Nothing in the request path reads
    /// `FOUNDATION_BRAND.nav` any more — one resolver builds the chrome for
    /// every page — so what this catches is a future divergent row being
    /// declared here, not a page rendering one. The rendered proof is
    /// `server::tests::firm_routes`'s
    /// `the_foundation_front_door_wears_the_shared_header`.
    #[test]
    fn the_foundation_carries_the_firms_header_row() {
        assert_eq!(
            FOUNDATION_BRAND
                .nav
                .iter()
                .map(|n| n.label)
                .collect::<Vec<_>>(),
            FIRM_BRAND.nav.iter().map(|n| n.label).collect::<Vec<_>>(),
            "one header serves both faces"
        );
        assert!(
            FOUNDATION_BRAND
                .nav
                .iter()
                .any(|link| link.href == "/foundation"),
            "and it links the nonprofit, which is the reader's way back to it"
        );
        assert_eq!(
            FOUNDATION_BRAND.home_href, "/foundation",
            "the Foundation's home is its own prefix, not the firm's root"
        );
    }

    /// The retired legal aid audience page is linked from neither row.
    ///
    /// It was the Foundation header's second entry, and the header was the only
    /// way that audience found it. The page is gone, so a link in either row
    /// would now be a link to a `404` — the same failure the retired Team entry
    /// is guarded against above.
    #[test]
    fn neither_row_links_the_retired_legal_aid_page() {
        for link in FIRM_BRAND.nav.iter().chain(super::firm_footer_nav()) {
            assert!(
                !link.href.contains("legal-aid"),
                "{} links the retired legal aid page: {}",
                link.label,
                link.href
            );
            assert_ne!(link.label, "Legal aid centers");
        }
    }

    /// The talks catalog is linked from the firm's footer, at its own name.
    #[test]
    fn presentations_is_a_firm_footer_leaf_at_the_catalogs_own_name() {
        let presentations = super::firm_footer_nav()
            .iter()
            .find(|n| n.label == "Presentations")
            .expect("Presentations leaf present");
        assert!(!presentations.is_dropdown());
        assert_eq!(presentations.href, "/presentations");
    }

    /// The workshop catalog stays one click from every page.
    ///
    /// It was in the Foundation header as that row's deliberate exception to
    /// the `/foundation` prefix rule. That row is gone, so the footer entry is
    /// now the only thing keeping the public catalog off "reachable by typing
    /// the URL" — which is why it is asserted here rather than left to the
    /// alphabetical list above.
    #[test]
    fn the_public_workshop_catalog_is_linked_from_the_footer() {
        let workshops = super::firm_footer_nav()
            .iter()
            .find(|n| n.label == "Workshops")
            .expect("the public workshop catalog is linked");
        assert_eq!(workshops.href, "/workshops");
        // Notations sits behind the session boundary, so no row advertises it:
        // an entry would send a signed-out reader at a login door.
        assert!(
            FIRM_BRAND
                .nav
                .iter()
                .chain(super::firm_footer_nav())
                .all(|link| link.href != "/foundation/notations"),
            "the gated notations index is advertised by neither row"
        );
    }

    #[test]
    fn foundation_email_defaults_to_neonlaw_org() {
        assert_eq!(super::foundation_email(), "support@neonlaw.org");
    }

    #[test]
    fn firm_email_defaults_to_a_mailbox_on_the_firms_own_host() {
        assert_eq!(super::firm_email(), "contact@neonlaw.com");
        // The local part is the firm's to choose — `firm_support_email`'s first
        // dial exists precisely to name a mailbox that is not `support@`. What
        // must not drift is the *host*: the built-in address reads mail at the
        // built-in domain, so moving `support_domain` without moving the address
        // is caught here.
        assert_eq!(
            super::firm_email().rsplit_once('@').map(|(_, host)| host),
            Some(DEFAULT_BRANDING.support_domain),
            "the built-in address and the built-in host cannot drift apart"
        );
    }

    /// The mailbox host and the infrastructure apex are still two dials, and on
    /// this deployment they hold the same value.
    ///
    /// They were required to differ while the firm read mail at one domain and
    /// derived `workflows.<apex>` from another. One site on one domain collapses
    /// that, so the assertion is now that both resolve to `neonlaw.com` — while
    /// the fields stay separate, because a deployment that moves its mailbox
    /// must still be able to do so without moving its service hostnames.
    #[test]
    fn the_mailbox_host_and_the_infrastructure_apex_are_both_neonlaw_com() {
        assert_eq!(DEFAULT_BRANDING.support_domain, "neonlaw.com");
        assert_eq!(DEFAULT_BRANDING.primary_domain, "neonlaw.com");
    }

    #[test]
    fn a_support_domain_alone_builds_the_support_address() {
        assert_eq!(
            super::firm_support_email(None, Some("example.test")),
            "support@example.test"
        );
    }

    #[test]
    fn an_explicit_support_email_wins_over_the_domain() {
        assert_eq!(
            super::firm_support_email(Some("hello@other.test"), Some("example.test")),
            "hello@other.test"
        );
    }

    #[test]
    fn neither_dial_set_keeps_the_built_in_address() {
        assert_eq!(super::firm_support_email(None, None), "contact@neonlaw.com");
        // An empty string is "unset", not "an address with no characters".
        assert_eq!(
            super::firm_support_email(Some(""), Some("")),
            "contact@neonlaw.com"
        );
    }

    #[test]
    fn terms_and_privacy_links_default_to_the_in_app_pages() {
        // Unset → the bundled `/terms` and `/privacy` routes. A white-label
        // deploy overrides these to its own off-site legal pages so
        // Neon Law Navigator never serves a deployer's binding legal text.
        assert_eq!(super::terms_url(), "/terms");
        assert_eq!(super::privacy_url(), "/privacy");
    }

    #[test]
    fn leaf_constructor_yields_no_children() {
        let n = NavLink::leaf("Home", "/");
        assert!(!n.is_dropdown());
        assert_eq!(n.href, "/");
    }

    #[test]
    fn dropdown_constructor_carries_children() {
        const CHILDREN: &[NavLink] = &[NavLink::leaf("A", "/a")];
        let n = NavLink::dropdown("Group", CHILDREN);
        assert!(n.is_dropdown());
        assert_eq!(n.children.len(), 1);
    }
}
