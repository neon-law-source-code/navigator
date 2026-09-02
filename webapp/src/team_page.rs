//! The firm's `/team` roster: the index and one profile per person.
//!
//! Two views share one chrome ([`TeamShell`]) and neither restates the
//! other's copy. The index names each person once and links to their own
//! page; a profile then says how to reach that one person — an email and a
//! `LinkedIn` profile — without repeating the roster's framing sentence. A
//! reader who wants to know who is here reads the index; a reader who wants
//! to reach someone specific reads their page.
//!
//! Both pages are live, per-request queries against
//! [`store::persons::find_team_members`] — any `Person` with a firm-side
//! role (not `client`) and a confirmed email — rather than a fixed roster
//! baked in at process boot. `/team/{slug}` is one generic route matched
//! against a slug computed from the current roster on every request (see
//! [`unique_slug`]); a bookmarked profile URL can therefore start pointing
//! at someone else if names collide across roster changes — an accepted
//! limitation of a name-derived, unpersisted slug.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    Avatar, BackBreadcrumb, ExternalLink, PublicShell, SiteHeader, SiteNavLink, SocialMeta,
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
    /// The avatar's public URL, or `None` to fall back to initials.
    pub avatar_url: Option<String>,
}

/// The `/team` index content: the roster, alphabetized by name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamIndexContent {
    pub head_title: String,
    pub meta_description: String,
    pub page_title: String,
    pub members: Vec<TeamMemberSummary>,
}

/// Everything the `/team` index renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamIndexView {
    pub chrome: PublicChrome,
    pub content: TeamIndexContent,
}

/// A [`store::persons::Person`] paired with the slug it was assigned within
/// the current roster — computed fresh on every call, never persisted. See
/// [`team_roster`].
#[cfg(feature = "server")]
struct TeamRosterEntry {
    person: store::persons::Person,
    slug: String,
}

/// The live `/team` roster: every confirmed, non-client `Person`
/// ([`store::persons::find_team_members`]), alphabetized, each paired with
/// a collision-safe slug computed from their name. Shared by the index and
/// profile loaders so both agree on one slug for a given request.
#[cfg(feature = "server")]
async fn team_roster() -> Result<Vec<TeamRosterEntry>, ServerFnError> {
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let people = store::persons::find_team_members(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut taken = std::collections::HashSet::new();
    Ok(people
        .into_iter()
        .map(|person| {
            let slug = unique_slug(&person.name, &mut taken);
            TeamRosterEntry { person, slug }
        })
        .collect())
}

/// Resolve the chrome from the process brand and the roster from
/// [`team_roster`].
#[cfg(feature = "server")]
async fn load_team_index() -> Result<TeamIndexView, ServerFnError> {
    let chrome = crate::public_chrome::firm_public_chrome_from_context().await;
    let firm_name = chrome.brand_name.clone();
    let members = team_roster()
        .await?
        .into_iter()
        .map(|entry| TeamMemberSummary {
            name: entry.person.name,
            href: format!("/team/{}", entry.slug),
            avatar_url: entry.person.profile_image_url,
        })
        .collect();
    Ok(TeamIndexView {
        chrome,
        content: TeamIndexContent {
            head_title: format!("{firm_name} | Team"),
            meta_description: format!("The people at {firm_name}, and how to reach each of them."),
            page_title: "Team".to_string(),
            members,
        },
    })
}

#[server]
pub async fn team_index_view() -> Result<TeamIndexView, ServerFnError> {
    load_team_index().await
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
                            a { class: "team-index__link", href: "{member.href}",
                                Avatar {
                                    name: member.name.clone(),
                                    image_url: member.avatar_url.clone(),
                                    size: 40,
                                    class: "team-index__avatar".to_string(),
                                }
                                "{member.name}"
                            }
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
    /// `None` when the person has no `LinkedIn` URL on file — the profile
    /// simply omits that row rather than linking a blank href.
    pub linkedin_href: Option<String>,
    pub avatar_url: Option<String>,
}

/// Everything one profile renders. `content: None` is a slug matching
/// nobody in the current roster — a genuine not-found, not a load error.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamProfileView {
    pub chrome: PublicChrome,
    pub content: Option<TeamProfileContent>,
}

/// Resolve the `{slug}` path parameter, look it up against the live
/// [`team_roster`], and build the profile — or commit a `404` and return
/// `content: None` when no current team member matches.
#[cfg(feature = "server")]
async fn load_team_profile() -> Result<TeamProfileView, ServerFnError> {
    let axum::extract::Path(slug) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    let chrome = crate::public_chrome::firm_public_chrome_from_context().await;
    let firm_name = chrome.brand_name.clone();

    let Some(entry) = team_roster()
        .await?
        .into_iter()
        .find(|entry| entry.slug == slug)
    else {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(TeamProfileView {
            chrome,
            content: None,
        });
    };
    let person = entry.person;

    Ok(TeamProfileView {
        chrome,
        content: Some(TeamProfileContent {
            head_title: format!("{firm_name} | {}", person.name),
            meta_description: format!("Reach {} at {firm_name}.", person.name),
            name: person.name,
            email: person.email,
            linkedin_href: person.linkedin_url,
            avatar_url: person.profile_image_url,
        }),
    })
}

#[server]
pub async fn team_profile_view() -> Result<TeamProfileView, ServerFnError> {
    load_team_profile().await
}

/// A `/team/{slug}` route entry.
#[component]
pub fn TeamProfileEntry() -> Element {
    let resource = use_server_future(team_profile_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    match view.content {
        Some(content) => rsx! {
            TeamProfilePage { chrome: view.chrome, content }
        },
        None => rsx! {
            TeamShell {
                chrome: view.chrome,
                title: "Not found".to_string(),
                description: String::new(),
                article { class: "team-profile",
                    BackBreadcrumb { href: "/team".to_string(), label: "All team profiles".to_string() }
                    h1 { "Team member not found" }
                }
            }
        },
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
                Avatar {
                    name: content.name.clone(),
                    image_url: content.avatar_url.clone(),
                    size: 96,
                    class: "team-profile__avatar".to_string(),
                }
                h1 { "{content.name}" }
                dl {
                    dt { "Email" }
                    dd {
                        a { href: "{mailto}", "{content.email}" }
                    }
                    if let Some(linkedin_href) = content.linkedin_href.clone() {
                        dt { "LinkedIn" }
                        dd {
                            ExternalLink { href: linkedin_href, "LinkedIn" }
                        }
                    }
                }
            }
        }
    }
}

/// A lowercase, hyphenated slug derived from `name`, unique against
/// `taken` — a second "Alex Kim" in the same roster becomes `alex-kim-2`,
/// a third `alex-kim-3`, and so on. Recomputed fresh on every request from
/// whoever currently qualifies for the roster; nothing about it is
/// persisted, so it is not a stable, bookmarkable identifier across a
/// roster change (see the module doc).
#[cfg(any(feature = "server", test))]
fn unique_slug(name: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() {
        "team-member".to_string()
    } else {
        base
    };

    let mut candidate = base.clone();
    let mut suffix = 2;
    while taken.contains(&candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_slug_lowercases_hyphenates_and_dedupes_collisions() {
        let mut taken = std::collections::HashSet::new();
        assert_eq!(unique_slug("Alex Kim", &mut taken), "alex-kim");
        assert_eq!(unique_slug("Alex Kim", &mut taken), "alex-kim-2");
        assert_eq!(unique_slug("Alex Kim", &mut taken), "alex-kim-3");
        assert_eq!(unique_slug("O'Brien, Sam!", &mut taken), "o-brien-sam");
    }

    #[test]
    fn unique_slug_falls_back_for_a_name_with_no_alphanumerics() {
        let mut taken = std::collections::HashSet::new();
        assert_eq!(unique_slug("   ", &mut taken), "team-member");
        assert_eq!(unique_slug("---", &mut taken), "team-member-2");
    }

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
                        avatar_url: None,
                    },
                    TeamMemberSummary {
                        name: "Nick".to_string(),
                        href: "/team/nick".to_string(),
                        avatar_url: Some("/assets/avatars/nick.png".to_string()),
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
            linkedin_href: Some(linkedin_href.to_string()),
            avatar_url: None,
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
    fn the_index_renders_an_avatar_image_when_set_and_initials_otherwise() {
        let out = index_html();
        assert!(
            out.contains(r#"src="/assets/avatars/nick.png""#),
            "Nick has an avatar_url set: {out}"
        );
        assert!(
            out.contains("team-index__avatar--initials"),
            "Jask has no avatar_url, so falls back to initials: {out}"
        );
        assert!(out.contains(">J<"), "{out}");
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
    fn a_profile_omits_the_linkedin_row_when_none_is_on_file() {
        fn app() -> Element {
            let content = TeamProfileContent {
                head_title: "Neon Law | Ada".to_string(),
                meta_description: "Reach Ada at Neon Law.".to_string(),
                name: "Ada".to_string(),
                email: "ada@example.com".to_string(),
                linkedin_href: None,
                avatar_url: None,
            };
            rsx! { TeamProfilePage { chrome: chrome(), content } }
        }
        let out = ssr(app);
        assert!(!out.contains("LinkedIn"), "{out}");
        assert!(out.contains("ada@example.com"), "{out}");
        assert!(
            out.contains("team-profile__avatar--initials"),
            "no avatar_url falls back to initials: {out}"
        );
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
