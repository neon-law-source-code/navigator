//! Client entry point for the wasm bundle.
//!
//! `dx` compiles this binary to `wasm32-unknown-unknown` with the `web`
//! feature; the campaign launches [`webapp::home::HomePageEntry`] to match its
//! server-rendered root, while other routes retain [`webapp::App`]. Navigator's server is the `web`
//! crate, not this binary — `web` links `webapp` as a library — so with no
//! platform feature selected (the workspace default) `main` is intentionally
//! empty and `cargo build --workspace` stays green.

fn main() {
    #[cfg(all(feature = "web", target_arch = "wasm32"))]
    {
        // Hydration must launch the same root used by the server's home router.
        let is_cyber_home = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("cyber-main"))
            .is_some();
        if is_cyber_home {
            // This campaign navigates with ordinary links, and its complete head
            // is rendered by the server. Preserve those tags during hydration;
            // the default browser document evaluates JavaScript to set a title,
            // which is incompatible with our strict Content Security Policy.
            dioxus::LaunchBuilder::web()
                .with_context_provider(|| {
                    Box::new(std::rc::Rc::new(
                        dioxus_fullstack_core::document::FullstackWebDocument::from(
                            dioxus::document::NoOpDocument,
                        ),
                    )
                        as std::rc::Rc<dyn dioxus::document::Document>)
                })
                .launch(webapp::home::HomePageEntry);
        } else {
            dioxus::launch(webapp::App);
        }
    }
}
