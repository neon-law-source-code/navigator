//! The public testimonials page, scoped to the house brand in the request.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta, TestimonialCards};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The brand key carried from the public router to the server function.
#[derive(Clone, Default)]
pub struct InjectedTestimonials {
    pub brand_key: String,
}

/// The public page's resolved data.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TestimonialsPageView {
    pub chrome: PublicChrome,
    pub testimonials: Vec<crate::components::TestimonialCard>,
}

/// Resolve the public testimonials that belong to the current house brand.
#[server]
pub async fn testimonials_page_view() -> Result<TestimonialsPageView, ServerFnError> {
    let injected =
        crate::public_chrome::copy_from_request_or_context(consume_context::<InjectedTestimonials>)
            .await;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let testimonials = store::testimonials::published_for_brand(&surreal, &injected.brand_key, 12)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
        .into_iter()
        .map(public_testimonial_card)
        .collect();
    Ok(TestimonialsPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        testimonials,
    })
}

#[cfg(feature = "server")]
fn public_testimonial_card(
    testimonial: store::testimonials::PublishedTestimonial,
) -> crate::components::TestimonialCard {
    crate::components::TestimonialCard {
        quote: testimonial.quote,
        attribution: testimonial.attribution_label.unwrap_or_default(),
        detail: None,
        profile_image_url: None,
        product_label: None,
    }
}

/// The public page entry.
#[component]
pub fn TestimonialsPageEntry() -> Element {
    let resource = use_server_future(testimonials_page_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        TestimonialsPage { chrome: view.chrome, testimonials: view.testimonials }
    }
}

/// The pure page. The heading remains when the store has no published rows;
/// cards and their grid are absent until a real client testimonial exists.
#[component]
pub fn TestimonialsPage(
    chrome: PublicChrome,
    #[props(default)] testimonials: Vec<crate::components::TestimonialCard>,
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
    let footer = rsx! { PublicFooter { chrome: chrome.clone() } };
    let title = format!("{} | Testimonials", chrome.brand_name);
    rsx! {
        document::Title { "{title}" }
        document::Meta { name: "description", content: "Testimonials" }
        SocialMeta {
            title: title.clone(),
            description: "Testimonials".to_string(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        PublicShell { header, footer,
            section { class: "testimonials-page", "aria-labelledby": "testimonials-title",
                h1 { id: "testimonials-title", "Testimonials" }
                TestimonialCards { cards: testimonials }
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

    #[test]
    fn an_empty_page_keeps_only_its_heading() {
        fn app() -> Element {
            rsx! { TestimonialsPage { chrome: PublicChrome::default() } }
        }

        let html = ssr(app);
        assert!(html.contains(">Testimonials<"), "{html}");
        assert!(!html.contains("testimonial-card"), "{html}");
        assert!(!html.contains("testimonial-grid"), "{html}");
    }
}
