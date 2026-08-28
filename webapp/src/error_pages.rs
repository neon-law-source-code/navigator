//! The 404, 403, and 500 pages, rendered with Dioxus (#956 Phase 4, #1009).
//!
//! These are unlike every other migrated page, and the difference drives the
//! design: **they are not routed.** A page behind a router gets its
//! `<!DOCTYPE>`, `<head>`, and `<body>` from `ServeConfig`, and its chrome from
//! a pre-layer. These are returned inline from roughly fifty handlers and
//! pre-layers across `portal`, so neither is available — `document::*` elements
//! are invisible to a bare `dioxus_ssr::render`, and there is no request-scoped
//! chrome to inject.
//!
//! So this module owns a document shell of its own ([`document`]) and resolves
//! chrome from the process brand rather than the request. That costs one real
//! thing, stated plainly: an error page's header shows the brand's own
//! destinations, not the host's. It is the same compromise the pages made
//! — `PageLayout` defaulted to the firm brand too — so nothing regresses here,
//! but it is a compromise rather than a property.
//!
//! Every page is static, so each is rendered once into a `String` and cloned
//! per response. A 404 should not cost a `VirtualDom` build.

use std::sync::LazyLock;

use axum::response::Html;
use dioxus::prelude::*;

use crate::components::{PublicShell, SiteHeader, SiteNavLink, THEME_STYLESHEET_HREF};
use crate::html_escape::escape_attr;
use crate::public_chrome::{ChromeNavLink, PublicChrome, PublicFooter};

/// Whether the visitor is signed in, which decides the header's utility group.
///
/// Deliberately narrower than `views::AuthState`: an error page only needs to
/// know "is there a session", not which tier it carries. The tier-specific
/// links belong on the pages that can act on them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Anonymous,
    SignedIn,
}

impl Viewer {
    fn utility(self) -> Vec<ChromeNavLink> {
        match self {
            Self::Anonymous => vec![ChromeNavLink {
                label: "Sign in".into(),
                href: "/auth/login".into(),
            }],
            Self::SignedIn => vec![
                ChromeNavLink {
                    label: "Portal".into(),
                    href: "/app/projects".into(),
                },
                ChromeNavLink {
                    label: "Sign out".into(),
                    href: "/auth/logout".into(),
                },
            ],
        }
    }
}

/// `404` — the page a slug lookup missed.
pub fn not_found() -> Html<String> {
    static ANON: LazyLock<String> = LazyLock::new(|| render(Kind::NotFound, Viewer::Anonymous));
    Html(ANON.clone())
}

/// `404` for a signed-in visitor, so the header still offers a way back into
/// the portal rather than stranding them on a dead end.
pub fn not_found_signed_in() -> Html<String> {
    static SIGNED_IN: LazyLock<String> = LazyLock::new(|| render(Kind::NotFound, Viewer::SignedIn));
    Html(SIGNED_IN.clone())
}

/// `403` — an authenticated visitor without the role a page requires, or an
/// IdP-authenticated email with no `persons` row.
pub fn forbidden(viewer: Viewer) -> Html<String> {
    static ANON: LazyLock<String> = LazyLock::new(|| render(Kind::Forbidden, Viewer::Anonymous));
    static SIGNED_IN: LazyLock<String> =
        LazyLock::new(|| render(Kind::Forbidden, Viewer::SignedIn));
    Html(match viewer {
        Viewer::Anonymous => ANON.clone(),
        Viewer::SignedIn => SIGNED_IN.clone(),
    })
}

/// `403` for the one case the generic [`forbidden`] page reads wrong: an
/// IdP-authenticated visitor whose email has no `persons` row.
///
/// Sign-up is operator-mediated, so this is not a misconfigured account — it is
/// someone who has not engaged the firm yet, and the generic "not authorized"
/// wording tells them nothing about what to do. The generic page keeps its
/// wording because it also serves a real lawyer who wandered into
/// `/admin`, whom this copy would be wrong for.
pub fn sign_in_not_provisioned() -> Html<String> {
    static PAGE: LazyLock<String> =
        LazyLock::new(|| render(Kind::SignInNotProvisioned, Viewer::Anonymous));
    Html(PAGE.clone())
}

/// `500` — deliberately generic. The underlying error never reaches the
/// browser.
pub fn server_error() -> Html<String> {
    static ANON: LazyLock<String> = LazyLock::new(|| render(Kind::ServerError, Viewer::Anonymous));
    Html(ANON.clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    NotFound,
    Forbidden,
    SignInNotProvisioned,
    ServerError,
}

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Self::NotFound => "Not found",
            Self::Forbidden => "Forbidden",
            Self::SignInNotProvisioned => "Portal access",
            Self::ServerError => "Server error",
        }
    }
}

/// Render one error page into a complete HTML document.
fn render(kind: Kind, viewer: Viewer) -> String {
    // The sign-in-denied page drops the utility group entirely. Its visitor
    // *just* authenticated, so the provider's SSO session is still live and a
    // "Sign in" link would carry them straight back to this same page — the
    // header would be offering the loop they are already stuck in. Their way
    // forward is the body's Contact link, not the chrome.
    let utility = match kind {
        Kind::SignInNotProvisioned => Vec::new(),
        _ => viewer.utility(),
    };
    let chrome = crate::public_chrome::firm_public_chrome(utility);
    let brand_name = chrome.brand_name.clone();
    let firm_name = chrome.firm_name.clone();
    // The support address comes from the brand seam, so a rebranded fork never
    // surfaces Neon Law's `support@` on its own 403.
    let support_email = views::brand::firm_email().to_string();

    let mut dom = VirtualDom::new_with_props(
        ErrorPage,
        ErrorPageProps {
            kind,
            chrome,
            firm_name,
            support_email,
        },
    );
    dom.rebuild_in_place();
    standalone_document(kind.title(), &brand_name, &dioxus_ssr::render(&dom))
}

/// The licensed GORP Serif faces, built once from the process asset origin.
///
/// `theme.css` names the family in `--nav-font-family` but cannot declare it:
/// the public repository carries the declaration, not the WOFF2 bytes, so the
/// `src` has to be resolved against the deployment's assets bucket at runtime.
///
/// A routed page receives this from `portal`'s `dioxus_document_head`
/// middleware. An error page cannot: it is returned from a pre-layer that sits
/// *outside* that middleware — deliberately, so a 404 costs no rendering work —
/// and from handlers on the top-level router, which the middleware never wraps
/// at all. So it declares the faces itself, from the same `views` source the
/// middleware uses, which is the drift the source is public to prevent.
static GORP_FACES: LazyLock<String> = LazyLock::new(|| {
    let regular = views::assets::asset_url("fonts/gorp-serif/GORPSerif-Regular.woff2");
    let bold = views::assets::asset_url("fonts/gorp-serif/GORPSerif-Bold.woff2");
    // The face block arrives already CSS-string-escaped from `views::layout`,
    // so it is interpolated raw exactly as both other render paths do; the
    // preload `href` is an attribute value and goes through `escape_attr`.
    let faces = views::assets::gorp_font_face_css(&regular, &bold);
    format!(
        "<link rel=\"preload\" as=\"font\" type=\"font/woff2\" crossorigin href=\"{}\">\
         <style>{faces}</style>",
        escape_attr(&regular)
    )
});

/// Wrap rendered body markup in a complete document.
///
/// Hand-built because there is no router to build it: `ServeConfig` is what
/// supplies this for every other page. Interpolated values go through
/// [`escape_attr`] rather than straight into `format!`, which escapes nothing.
/// Wrap server-rendered Dioxus markup in the standalone document shell used by
/// inline handlers that do not pass through a Dioxus router.
#[must_use]
pub fn standalone_document(title: &str, brand_name: &str, body_html: &str) -> String {
    let full_title = escape_attr(&format!("{brand_name} | {title}"));
    let stylesheet = escape_attr(THEME_STYLESHEET_HREF);
    // The faces go last in the head, not first: `<meta charset>` must stay
    // within the document's first 1024 bytes, and a face block carrying two
    // absolute bucket URLs is large enough to push it out.
    let faces = &*GORP_FACES;
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head>\
         <meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <meta name=\"robots\" content=\"noindex\">\
         <title>{full_title}</title>\
         <link rel=\"stylesheet\" href=\"{stylesheet}\">\
         {faces}\
         </head><body>{body_html}</body></html>"
    )
}

#[derive(Props, Clone, PartialEq)]
struct ErrorPageProps {
    kind: Kind,
    chrome: PublicChrome,
    firm_name: String,
    support_email: String,
}

#[component]
fn ErrorPage(props: ErrorPageProps) -> Element {
    let ErrorPageProps {
        kind,
        chrome,
        firm_name,
        support_email,
    } = props;
    let header = rsx! {
        SiteHeader {
            brand_name: chrome.brand_name.clone(),
            home_href: chrome.home_href.clone(),
            logo_href: chrome.logo_href.clone(),
            destinations: chrome
                .destinations
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
            utility: chrome
                .utility
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
        }
    };
    let footer = rsx! {
        PublicFooter { chrome: chrome.clone() }
    };
    rsx! {
        PublicShell { header, footer,
            section { class: "error-page",
                match kind {
                    Kind::NotFound => rsx! {
                        h1 { "Not found" }
                        p { "The page you asked for does not exist." }
                    },
                    Kind::Forbidden => rsx! {
                        h1 { "Forbidden" }
                        p {
                            "Your account is not authorized to view this page. If you think this is a mistake, contact "
                            a { href: "mailto:{support_email}", "{support_email}" }
                            "."
                        }
                    },
                    Kind::SignInNotProvisioned => rsx! {
                        h1 { "You don't have portal access yet" }
                        p {
                            "The portal is for clients who have already engaged {firm_name}. "
                            "We didn't find an account for the email address you signed in with."
                        }
                        p {
                            "If you'd like to work with us, get in touch and we'll take it from there."
                        }
                        p {
                            "Already a client? You may have signed in with a different address than the one we have on file — write to "
                            a { href: "mailto:{support_email}", "{support_email}" }
                            " and we'll sort it out."
                        }
                    },
                    Kind::ServerError => rsx! {
                        h1 { "Something went wrong" }
                        p { "We hit an unexpected error. The team has been notified." }
                    },
                }
                p {
                    if kind == Kind::SignInNotProvisioned {
                        a { class: "nav-btn nav-btn--primary", href: "mailto:contact@neonlaw.com", "Contact us" }
                    }
                    a { class: "nav-btn nav-btn--secondary", href: "/", "Return home" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        forbidden, not_found, not_found_signed_in, server_error, sign_in_not_provisioned,
        standalone_document, Viewer,
    };

    fn body_of(html: axum::response::Html<String>) -> String {
        html.0
    }

    #[test]
    fn each_page_is_a_complete_document_with_its_own_title() {
        // The whole reason this module hand-builds a shell: nothing else does.
        for (html, title) in [
            (body_of(not_found()), "Not found"),
            (body_of(forbidden(Viewer::Anonymous)), "Forbidden"),
            (body_of(sign_in_not_provisioned()), "Portal access"),
            (body_of(server_error()), "Server error"),
        ] {
            assert!(html.starts_with("<!DOCTYPE html>"), "doctype: {html}");
            assert!(html.contains("<html lang=\"en\">"), "lang: {html}");
            assert!(
                html.contains(&format!("| {title}</title>")),
                "title {title}: {html}"
            );
            assert!(html.contains("/public/css/theme.css"), "styles: {html}");
            assert!(html.ends_with("</html>"), "closed: {html}");
        }
    }

    #[test]
    fn every_error_page_offers_a_way_out() {
        for html in [
            body_of(not_found()),
            body_of(forbidden(Viewer::Anonymous)),
            body_of(sign_in_not_provisioned()),
            body_of(server_error()),
        ] {
            assert!(html.contains(">Return home<"), "escape hatch: {html}");
        }
    }

    #[test]
    fn error_pages_are_not_indexable() {
        // A 404 that Google indexes is a 404 people arrive at from search.
        assert!(body_of(not_found()).contains(r#"<meta name="robots" content="noindex">"#));
    }

    #[test]
    fn the_signed_in_404_keeps_a_route_back_into_the_portal() {
        // The page took an `AuthState` for exactly this, and every one of
        // its call sites passed `Authenticated` — so the distinction is real
        // even though the parameter was not.
        let anon = body_of(not_found());
        let signed_in = body_of(not_found_signed_in());
        assert!(anon.contains("/auth/login"), "anonymous is offered sign-in");
        assert!(
            !anon.contains("/auth/logout"),
            "anonymous is not offered sign-out: {anon}"
        );
        assert!(
            signed_in.contains("/app/projects") && signed_in.contains("/auth/logout"),
            "signed-in keeps portal + sign-out: {signed_in}"
        );
    }

    #[test]
    fn the_403_names_a_support_address_from_the_brand_seam() {
        let html = body_of(forbidden(Viewer::SignedIn));
        assert!(html.contains("mailto:"), "a way to appeal: {html}");
        assert!(
            html.contains("not authorized"),
            "says what happened: {html}"
        );
    }

    #[test]
    fn the_sign_in_denial_explains_the_portal_is_for_existing_clients() {
        // The whole reason this page exists: the generic 403 said "not
        // authorized", which reads as a broken account to someone who has
        // simply never engaged the firm.
        let html = body_of(sign_in_not_provisioned());
        assert!(
            html.contains("already engaged"),
            "names the precondition: {html}"
        );
        // Matched without the apostrophe: the renderer escapes it to an HTML
        // entity, so asserting on the raw character pins an encoding detail
        // rather than the copy.
        assert!(
            html.contains("find an account for the email address"),
            "says what happened: {html}"
        );
        assert!(
            !html.contains("not authorized"),
            "does not fall back to the generic wording: {html}"
        );
    }

    #[test]
    fn the_sign_in_denial_offers_contact_as_the_way_forward() {
        // "Contact us if you want to work with us" is the action the page is
        // for; a mailto covers the client who used the wrong address.
        let html = body_of(sign_in_not_provisioned());
        // The CTA's own text, not just `href="/contact"` — the header's nav
        // carries that same href, so matching the bare link would pass even
        // with the button deleted.
        assert!(html.contains(">Contact us<"), "contact CTA: {html}");
        assert!(html.contains("mailto:"), "wrong-address path: {html}");
    }

    #[test]
    fn the_sign_in_denial_does_not_offer_the_sign_in_loop() {
        // The visitor authenticated moments ago, so the provider's SSO session
        // is still live: a "Sign in" link would land them right back here.
        let html = body_of(sign_in_not_provisioned());
        assert!(
            !html.contains("/auth/login"),
            "no loop back into sign-in: {html}"
        );
    }

    #[test]
    fn the_generic_403_keeps_its_own_wording() {
        // It also serves a real lawyer who wandered into `/admin`, for
        // whom "engage us as a client" would be wrong.
        let html = body_of(forbidden(Viewer::SignedIn));
        assert!(html.contains("not authorized"), "unchanged: {html}");
        assert!(
            !html.contains("already engaged"),
            "the client-facing copy did not leak into the generic page: {html}"
        );
    }

    #[test]
    fn the_500_never_leaks_the_underlying_error() {
        // Generic by construction: nothing about the failure reaches the page,
        // so there is no path by which a message could.
        let html = body_of(server_error());
        assert!(html.contains("Something went wrong"));
        assert!(html.contains("unexpected error"));
    }

    /// Every error page, with the name a failure should report it under.
    ///
    /// `not_found_signed_in` and `forbidden(SignedIn)` render a different
    /// header from their anonymous twins (the utility group carries portal +
    /// sign-out rather than sign-in), so both arms are audited rather than
    /// assuming the shell is shared.
    fn every_page() -> Vec<(&'static str, String)> {
        vec![
            ("404 (anonymous)", body_of(not_found())),
            ("404 (signed in)", body_of(not_found_signed_in())),
            ("403 (anonymous)", body_of(forbidden(Viewer::Anonymous))),
            ("403 (signed in)", body_of(forbidden(Viewer::SignedIn))),
            ("403 (not provisioned)", body_of(sign_in_not_provisioned())),
            ("500", body_of(server_error())),
        ]
    }

    /// The error pages carry the accessibility contract axe enforces on every
    /// routed page — but no browser gate can reach them, because **they are not
    /// routed**. `server/tests/accessibility_e2e.rs` can drive a 404 (any
    /// unknown path) and a 403 (a client who opens a lawyer route), and does;
    /// it can reach the 500 only by breaking the server, and it can reach
    /// neither `not_found_signed_in` nor `sign_in_not_provisioned` at all
    /// without fabricating state. So the contract is pinned here, on the
    /// rendered string, where every page is one function call away and the
    /// regression is caught in the ordinary workspace run.
    ///
    /// These four are the document-level rules axe reports as `html-has-lang`,
    /// `document-title`, `landmark-one-main`, and `page-has-heading-one` —
    /// the same checks the live gate runs against the brand shells.
    #[test]
    fn every_error_page_meets_the_document_accessibility_contract() {
        for (name, html) in every_page() {
            assert!(
                html.contains("<html lang=\"en\">"),
                "{name}: a screen reader picks the voice from `lang`: {html}"
            );
            assert_eq!(
                html.matches("<title>").count(),
                1,
                "{name}: exactly one document title: {html}"
            );
            assert_eq!(
                html.matches("<main").count(),
                1,
                "{name}: exactly one main landmark, so \"skip to content\" has \
                 one unambiguous target: {html}"
            );
            assert_eq!(
                html.matches("<h1").count(),
                1,
                "{name}: exactly one first-level heading naming the page: {html}"
            );
            // A named primary nav is what lets a screen-reader user tell the
            // site navigation apart from any other list of links on the page.
            assert!(
                html.contains("aria-label=\"Primary\""),
                "{name}: the primary navigation landmark is named: {html}"
            );
            assert!(
                html.contains("<footer"),
                "{name}: the shell's footer landmark: {html}"
            );
        }
    }

    /// axe reports `meta-viewport` when a page pins `user-scalable=no` or caps
    /// `maximum-scale`, because that blocks a low-vision reader from zooming.
    /// The shell is hand-built here rather than supplied by `ServeConfig`, so
    /// nothing else would catch a regression in it.
    #[test]
    fn every_error_page_lets_the_reader_zoom() {
        for (name, html) in every_page() {
            assert!(
                html.contains(
                    r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#
                ),
                "{name}: a scalable viewport: {html}"
            );
            assert!(
                !html.contains("user-scalable=no") && !html.contains("maximum-scale"),
                "{name}: zoom may not be capped: {html}"
            );
        }
    }

    /// The shell declares `color-scheme: light dark`, which is what makes the
    /// browser paint its own form controls and scrollbars for the active
    /// scheme. Without it a dark-theme page draws light native chrome on a
    /// dark ground — a real contrast failure the two-scheme browser audit in
    /// `accessibility_e2e.rs` covers for routed pages, and this covers here.
    #[test]
    fn every_error_page_declares_both_colour_schemes() {
        for (name, html) in every_page() {
            assert!(
                html.contains(r#"<meta name="color-scheme" content="light dark">"#),
                "{name}: both schemes declared: {html}"
            );
        }
    }

    #[test]
    fn every_error_page_declares_the_licensed_faces_itself() {
        // `theme.css` names "GORP Serif" in `--nav-font-family` but cannot
        // declare it — the WOFF2 bytes live in the deployment's asset bucket.
        // A routed page gets the declaration from `portal`'s
        // `dioxus_document_head` middleware, but every error page is returned
        // from a pre-layer that sits *outside* that middleware, so without this
        // the whole family falls back to Georgia.
        for html in [
            body_of(not_found()),
            body_of(forbidden(Viewer::Anonymous)),
            body_of(server_error()),
        ] {
            assert!(html.contains("@font-face"), "faces declared: {html}");
            assert!(html.contains("GORP Serif"), "the firm family: {html}");
            assert!(
                html.contains("rel=\"preload\""),
                "reading face preloaded so first paint is not a fallback flash: {html}"
            );
        }
    }

    #[test]
    fn the_document_shell_escapes_what_it_interpolates() {
        // `format!` escapes nothing, so the shell's own interpolation is a real
        // injection surface even though today's callers pass literals.
        let html = standalone_document("</title><script>alert(1)</script>", "Brand", "<p>body</p>");
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "a crafted title cannot close the tag and open a script: {html}"
        );
        assert!(
            html.contains("<p>body</p>"),
            "the body is inserted as markup"
        );
    }
}
