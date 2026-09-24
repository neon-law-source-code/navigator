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
//!
//! Every block, highlighted or plain, carries a copy button. The button is
//! inert markup: `script-src` forbids an inline handler, and
//! [`COPY_CODE_SCRIPT_HREF`] (deduped per page) performs the copy. Markdown
//! that never passes through this component emits the same button from
//! `views::components::code::with_copy_button`, and the document middleware
//! loads the script when that hook is present.

use dioxus::prelude::*;

/// Same-origin script that copies the source of a `[data-copy-code]` button's
/// block. Dioxus dedupes `document::Script` by this `src`.
pub const COPY_CODE_SCRIPT_HREF: &str = "/public/js/copy-code.js";

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

/// The copy control shared by [`CodeBlock`] and [`PlainCodeBlock`].
///
/// The visible label and the accessible name match
/// `views::components::code::COPY_BUTTON_OPEN`, so a block built in `rsx!` and
/// a block built as HTML offer the same control.
#[component]
fn CopyCodeButton() -> Element {
    rsx! {
        document::Script { src: COPY_CODE_SCRIPT_HREF, defer: true }
        button {
            class: "nav-code__copy",
            r#type: "button",
            "data-copy-code": "true",
            "aria-label": "Copy code",
            "Copy"
        }
    }
}

/// A code block, syntax-highlighted server-side. `lang` defaults to `rust`.
/// The highlighted HTML resolves during SSR and is rendered as inner HTML
/// (syntect's token text is already HTML-escaped). Before the highlight
/// resolves — or if it fails — the plain, escaped source is shown, so the
/// code is always readable. Either way the block has a copy button.
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
    rsx! {
        div { class: "nav-code",
            CopyCodeButton {}
            if let Some(html) = highlighted {
                // The highlight is a `<pre><code>`, not a wrapper. Putting it
                // on this div would replace the button: `dangerous_inner_html`
                // is the element's entire content.
                div { class: "nav-code__source", dangerous_inner_html: "{html}" }
            } else {
                pre { tabindex: "0", code { "{fallback}" } }
            }
        }
    }
}

/// A code block that stays plain text, with the same copy button as [`CodeBlock`].
///
/// Used where colouring the source would be the wrong reading: a shell command
/// a reader pastes as-is, a notation specimen, a template's verbatim frontmatter.
/// `class` lands on the `<pre>` so the host page can keep its own type treatment.
#[component]
pub(crate) fn PlainCodeBlock(code: String, #[props(default)] class: String) -> Element {
    rsx! {
        div { class: "nav-code",
            CopyCodeButton {}
            if class.is_empty() {
                pre { tabindex: "0", code { "{code}" } }
            } else {
                pre { class: "{class}", tabindex: "0", code { "{code}" } }
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
    fn a_code_block_renders_a_copy_button_beside_the_source() {
        fn app() -> Element {
            rsx! { CodeBlock { code: "let x = 1;".to_string() } }
        }
        let out = ssr(app);
        assert!(out.contains("nav-code"), "wrapper: {out}");
        assert!(out.contains("data-copy-code=\"true\""), "copy hook: {out}");
        assert!(
            out.contains("aria-label=\"Copy code\""),
            "accessible name: {out}"
        );
        assert!(out.contains(">Copy<"), "visible label: {out}");
        assert!(
            out.contains("let") && out.contains('1'),
            "source is readable: {out}"
        );
        assert!(!out.contains("onclick"), "no inline handler: {out}");
        assert_eq!(
            out.matches("data-copy-code").count(),
            1,
            "one button: {out}"
        );
    }

    #[test]
    fn a_plain_block_keeps_its_host_class_and_the_copy_button() {
        fn app() -> Element {
            rsx! {
                PlainCodeBlock {
                    code: "brew install navigator".to_string(),
                    class: "fm-package__command".to_string(),
                }
            }
        }
        let out = ssr(app);
        assert!(
            out.contains("class=\"fm-package__command\""),
            "host class stays on the pre: {out}"
        );
        assert!(out.contains("data-copy-code=\"true\""), "copy hook: {out}");
        assert!(
            out.contains("brew install navigator"),
            "command text: {out}"
        );
    }

    /// `.nav-code pre` (and page overrides like `.notations-specimen pre`, used
    /// by the `/notations` specimen this component renders) set
    /// `overflow-x: auto`, so the block must be keyboard reachable itself or
    /// axe's `scrollable-region-focusable` rule fails
    /// (`server/tests/accessibility_e2e.rs`).
    #[test]
    fn a_plain_block_is_keyboard_focusable_with_or_without_a_host_class() {
        fn unclassed() -> Element {
            rsx! { PlainCodeBlock { code: "brew install navigator".to_string() } }
        }
        fn classed() -> Element {
            rsx! {
                PlainCodeBlock {
                    code: "brew install navigator".to_string(),
                    class: "fm-package__command".to_string(),
                }
            }
        }
        for (out, label) in [(ssr(unclassed), "unclassed"), (ssr(classed), "classed")] {
            assert!(
                out.contains("tabindex=\"0\""),
                "{label} pre carries a tabindex: {out}"
            );
        }
    }

    /// [`CodeBlock`]'s escaped fallback (rendered before the server highlight
    /// resolves, or if it fails) must be just as keyboard reachable as
    /// [`PlainCodeBlock`].
    #[test]
    fn a_code_block_fallback_is_keyboard_focusable() {
        fn app() -> Element {
            rsx! { CodeBlock { code: "let x = 1;".to_string() } }
        }
        let out = ssr(app);
        assert!(
            out.contains("tabindex=\"0\""),
            "fallback pre carries a tabindex: {out}"
        );
    }

    /// The script the button depends on is the file we ship, and it copies
    /// through the same hook the markup emits.
    #[test]
    fn the_copy_script_uses_the_button_hook_and_the_clipboard() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../server/public/js/copy-code.js");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        assert!(
            source.contains("[data-copy-code]"),
            "script must listen for the button hook"
        );
        assert!(
            source.contains("navigator.clipboard.writeText"),
            "script must use the clipboard API"
        );
        assert!(!source.contains("eval("), "no eval: {source}");
        assert!(
            !source.contains("onclick"),
            "the behavior stays out of inline handlers"
        );
    }
}
