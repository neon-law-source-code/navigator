//! Run-marked prose: a paragraph the firm sets partly in bold, carried as data
//! rather than as raw HTML.
//!
//! The wire shape of a marked-up paragraph, kept wasm-safe. Marketing prose —
//! `/design`'s showcase and the practice pages — sets the emphasis flag on a
//! run to bold a phrase without accepting raw HTML.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One run of prose. `emphasis` renders it as `<strong>`; everything else is
/// plain text, so no page has to accept raw HTML to keep the typography.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CopyRun {
    pub text: String,
    pub emphasis: bool,
}

/// Map the owned `(text, emphasis)` pairs a content module hands a `#[server]`
/// function onto the wire runs.
#[must_use]
pub fn wire_runs(runs: Vec<(String, bool)>) -> Vec<CopyRun> {
    runs.into_iter()
        .map(|(text, emphasis)| CopyRun { text, emphasis })
        .collect()
}

/// One paragraph of run-marked prose: plain text, with the firm's emphasised
/// phrases in `<strong>`.
#[component]
pub fn RunParagraph(class: String, runs: Vec<CopyRun>) -> Element {
    rsx! {
        p { class: "{class}",
            for run in runs.iter() {
                if run.emphasis {
                    strong { "{run.text}" }
                } else {
                    "{run.text}"
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

    #[test]
    fn emphasised_runs_render_as_strong_and_plain_runs_do_not() {
        fn app() -> Element {
            rsx! {
                RunParagraph {
                    class: "bio".to_string(),
                    runs: wire_runs(vec![
                        ("Jacob specializes in ".to_string(), false),
                        ("recovering losses".to_string(), true),
                        (".".to_string(), false),
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains(r#"class="bio""#), "{html}");
        assert!(
            html.contains("<strong>recovering losses</strong>"),
            "{html}"
        );
        assert!(!html.contains("<strong>Jacob specializes in"), "{html}");
    }
}
