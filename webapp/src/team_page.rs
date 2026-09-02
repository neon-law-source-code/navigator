//! The firm's `/team` roster: the index and one profile per person.
//!
//! Two views share one chrome ([`TeamShell`]) and neither restates the
//! other's copy. The index names each person once and links to their own
//! page; a profile then says how to reach that one person — an email and a
//! `LinkedIn` profile — without repeating the roster's framing sentence. A
//! reader who wants to know who is here reads the index; a reader who wants
//! to reach someone specific reads their page.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    BackBreadcrumb, ExternalLink, PublicShell, SiteHeader, SiteNavLink, SocialMeta,
};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The chrome both team views wrap their content in.
#[component]
fn TeamShell(
    chrome: PublicChrome,
    title: String,
    description: String,
    children: Element,
) -> Element {
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
        document::Title { "{title}" }
        document::Meta { name: "description", content: "{description}" }
        SocialMeta {
            title: title.clone(),
            description: description.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        PublicShell { header, footer, {children} }
    }
}

/// One entry on the `/team` index: a name and the profile it links to.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamMemberSummary {
    pub name: String,
    pub href: String,
}

/// The `/team` index content: the roster, alphabetized by name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamIndexContent {
    pub head_title: String,
    pub meta_description: String,
    pub page_title: String,
    pub members: Vec<TeamMemberSummary>,
}

/// The [`TeamIndexContent`] injected into the render context by the portal
/// router.
#[derive(Clone, Default)]
pub struct InjectedTeamIndex(pub TeamIndexContent);

/// Everything the `/team` index renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamIndexView {
    pub chrome: PublicChrome,
    pub content: TeamIndexContent,
}

/// Resolve the chrome from the process brand and the roster from the
/// injected [`InjectedTeamIndex`] context.
#[server]
pub async fn team_index_view() -> Result<TeamIndexView, ServerFnError> {
    let content = consume_context::<InjectedTeamIndex>().0;
    Ok(TeamIndexView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The `/team` index route entry.
#[component]
pub fn TeamIndexEntry() -> Element {
    let resource = use_server_future(team_index_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        TeamIndexPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure `/team` index page. Prop-driven, so it server-renders and
/// unit-tests without a server future.
#[component]
pub fn TeamIndexPage(chrome: PublicChrome, content: TeamIndexContent) -> Element {
    rsx! {
        TeamShell {
            chrome: chrome.clone(),
            title: content.head_title.clone(),
            description: content.meta_description.clone(),
            article { class: "team-index",
                h1 { "{content.page_title}" }
                ul { class: "team-index__list",
                    for member in content.members.iter() {
                        li { class: "team-index__item", key: "{member.href}",
                            a { class: "team-index__link", href: "{member.href}", "{member.name}" }
                        }
                    }
                }
            }
        }
    }
}

/// One person's `/team/{slug}` content: how to reach them.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamProfileContent {
    pub head_title: String,
    pub meta_description: String,
    pub name: String,
    pub email: String,
    pub linkedin_href: String,
}

/// The [`TeamProfileContent`] injected into the render context by the portal
/// router. Each profile router injects its own instance, so `/team/nick` and
/// `/team/jask` share this one component with different content.
#[derive(Clone, Default)]
pub struct InjectedTeamProfile(pub TeamProfileContent);

/// Everything one profile renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamProfileView {
    pub chrome: PublicChrome,
    pub content: TeamProfileContent,
}

/// Resolve the chrome from the process brand and the profile from the
/// injected [`InjectedTeamProfile`] context.
#[server]
pub async fn team_profile_view() -> Result<TeamProfileView, ServerFnError> {
    let content = consume_context::<InjectedTeamProfile>().0;
    Ok(TeamProfileView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// A `/team/{slug}` route entry.
#[component]
pub fn TeamProfileEntry() -> Element {
    let resource = use_server_future(team_profile_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        TeamProfilePage { chrome: view.chrome, content: view.content }
    }
}

/// The pure profile page: a name, and the two ways to reach them. Prop-driven,
/// so it server-renders and unit-tests without a server future.
#[component]
pub fn TeamProfilePage(chrome: PublicChrome, content: TeamProfileContent) -> Element {
    let mailto = format!("mailto:{}", content.email);
    rsx! {
        TeamShell {
            chrome: chrome.clone(),
            title: content.head_title.clone(),
            description: content.meta_description.clone(),
            article { class: "team-profile",
                BackBreadcrumb { href: "/team".to_string(), label: "All team profiles".to_string() }
                h1 { "{content.name}" }
                dl {
                    dt { "Email" }
                    dd {
                        a { href: "{mailto}", "{content.email}" }
                    }
                    dt { "LinkedIn" }
                    dd {
                        ExternalLink { href: content.linkedin_href.clone(), "LinkedIn" }
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

    fn chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Neon Law".to_string(),
            home_href: "/".to_string(),
            logo_href: "/public/logo.svg".to_string(),
            social_image: "https://example.test/og.png".to_string(),
            ..PublicChrome::default()
        }
    }

    fn index_html() -> String {
        fn app() -> Element {
            let content = TeamIndexContent {
                head_title: "Neon Law | Team".to_string(),
                meta_description: "The firm's team.".to_string(),
                page_title: "Team".to_string(),
                members: vec![
                    TeamMemberSummary {
                        name: "Jask".to_string(),
                        href: "/team/jask".to_string(),
                    },
                    TeamMemberSummary {
                        name: "Nick".to_string(),
                        href: "/team/nick".to_string(),
                    },
                ],
            };
            rsx! { TeamIndexPage { chrome: chrome(), content } }
        }
        ssr(app)
    }

    fn profile_html(name: &str, email: &str, linkedin_href: &str) -> String {
        fn app_with(content: TeamProfileContent) -> Element {
            rsx! { TeamProfilePage { chrome: chrome(), content } }
        }
        let content = TeamProfileContent {
            head_title: format!("Neon Law | {name}"),
            meta_description: format!("Reach {name} at Neon Law."),
            name: name.to_string(),
            email: email.to_string(),
            linkedin_href: linkedin_href.to_string(),
        };
        let mut dom = VirtualDom::new_with_props(app_with, content);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn the_index_links_every_member_to_their_own_profile() {
        let out = index_html();
        assert!(out.contains(r#"href="/team/nick""#), "{out}");
        assert!(out.contains(r#"href="/team/jask""#), "{out}");
        assert!(out.contains(">Nick<"), "{out}");
        assert!(out.contains(">Jask<"), "{out}");
    }

    #[test]
    fn the_index_wraps_in_the_public_shell_chrome() {
        let out = index_html();
        assert!(out.contains("site-header"), "{out}");
        assert!(out.contains("site-footer__legal"), "{out}");
    }

    #[test]
    fn a_profile_names_the_email_and_links_it_as_mailto() {
        let out = profile_html(
            "Nick",
            "nick@neonlaw.com",
            "https://www.linkedin.com/in/nicholas-shook/",
        );
        assert!(out.contains("nick@neonlaw.com"), "{out}");
        assert!(
            out.contains(r#"href="mailto:nick@neonlaw.com""#),
            "the email is a mailto link: {out}"
        );
    }

    #[test]
    fn a_profile_links_linkedin_off_site_with_the_owasp_rel_pair() {
        let out = profile_html(
            "Nick",
            "nick@neonlaw.com",
            "https://www.linkedin.com/in/nicholas-shook/",
        );
        assert!(
            out.contains(r#"href="https://www.linkedin.com/in/nicholas-shook/""#),
            "{out}"
        );
        assert!(out.contains(r#"target="_blank""#), "{out}");
        assert!(out.contains(r#"rel="noopener noreferrer""#), "{out}");
    }

    #[test]
    fn a_profile_breadcrumbs_back_to_the_index() {
        let out = profile_html(
            "Jask",
            "jask@neonlaw.com",
            "https://www.linkedin.com/in/jasks/",
        );
        assert!(out.contains(r#"href="/team""#), "{out}");
        assert!(out.contains("All team profiles"), "{out}");
    }

    #[test]
    fn a_profile_wraps_in_the_public_shell_chrome() {
        let out = profile_html(
            "Nick",
            "nick@neonlaw.com",
            "https://www.linkedin.com/in/nicholas-shook/",
        );
        assert!(out.contains("site-header"), "{out}");
        assert!(out.contains("site-footer__legal"), "{out}");
    }
}
