//! A small profile picture, rendered as an `<img>` when a URL is set, or a
//! generated-initials circle otherwise. Shared by [`crate::components::
//! testimonial`], [`crate::team_page`], and the admin [`crate::person_show`]
//! avatar preview, so the three surfaces agree on one fallback.

use dioxus::prelude::*;

/// A profile picture (or its initials fallback), sized in pixels.
#[component]
pub fn Avatar(name: String, image_url: Option<String>, size: u32, class: String) -> Element {
    let dimension = size.to_string();
    rsx! {
        if let Some(url) = image_url {
            img {
                class: "{class}",
                src: "{url}",
                alt: "{name} profile image",
                width: "{dimension}",
                height: "{dimension}",
            }
        } else {
            div {
                class: "{class} {class}--initials",
                "aria-hidden": "true",
                "{initials(&name)}"
            }
        }
    }
}

/// Up-to-two-letter initials for the avatar fallback. Falls back to `"N"`
/// when `name` is empty or whitespace-only, so a blank name still renders a
/// filled avatar rather than an empty circle.
#[must_use]
pub fn initials(name: &str) -> String {
    let out: String = name
        .split_whitespace()
        .take(2)
        .filter_map(|part| part.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if out.is_empty() {
        "N".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn takes_up_to_two_leading_letters_uppercased() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("cher"), "C");
        assert_eq!(initials("  "), "N");
        assert_eq!(initials(""), "N");
        assert_eq!(initials("Madonna Ciccone Extra"), "MC");
    }
}
