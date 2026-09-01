//! Server-side syntax-highlighted code block, as a Dioxus component (issue
//! #641, Phase 2).
//!
//! The successor to the `views::components::code`. Highlighting runs on the
//! **server** through `syntect` (pure Rust, not wasm-safe), so [`highlight_code`]
//! is a `#[server]` function — its body reuses the tested `views` highlighter and
//! the wasm client calls the generated HTTP stub. The coloured `<pre><code>` is
//! inline-styled, so it needs only `style-src 'unsafe-inline'` (already in the
//! CSP) and no vendored client highlighter. [`use_server_future`] resolves the
//! highlight during SSR, so the coloured markup is in the pre-hydration HTML.
//!
//! `lang` is a fence token (`rust`, `yaml`, `toml`, `bash`, …) resolved
//! against `syntect`'s bundled syntax set; an unrecognised or empty one
//! renders as plain, uncoloured (but still readable) text.

use dioxus::prelude::*;

/// Highlight `code` as `lang` into an inline-styled `<pre><code>`,
/// server-side. The body runs only on the server (`syntect` does not compile
/// to `wasm32`); it reuses `views::components::code`, the same highlighter
/// the pages use.
#[server]
// A server function must be `async` (the macro requires it); this one highlights
// synchronously, with nothing to await.
#[allow(clippy::unused_async)]
pub async fn highlight_code(code: String, lang: String) -> Result<String, ServerFnError> {
    Ok(views::components::code::highlight(&code, &lang))
}

/// A code block, syntax-highlighted server-side. `lang` defaults to `rust`.
/// The highlighted HTML resolves during SSR and is rendered as inner HTML
/// (syntect's token text is already HTML-escaped). Before the highlight
/// resolves — or if it fails — the plain, escaped source is shown, so the
/// code is always readable.
#[component]
pub fn CodeBlock(code: String, #[props(default = "rust".to_string())] lang: String) -> Element {
    let fallback = code.clone();
    let resource = use_server_future(move || highlight_code(code.clone(), lang.clone()))?;
    // Clone the highlighted HTML out of the read guard before rendering so the
    // borrow does not outlive it (the `rsx!` output escapes this scope).
    let highlighted = match &*resource.read() {
        Some(Ok(html)) => Some(html.clone()),
        _ => None,
    };
    match highlighted {
        Some(html) => rsx! {
            div { class: "nav-code", dangerous_inner_html: html }
        },
        None => rsx! {
            div { class: "nav-code",
                pre { code { "{fallback}" } }
            }
        },
    }
}
