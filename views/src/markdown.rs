//! `CommonMark` → HTML rendering for static prose pages.
//!
//! Prose bodies live in each deployment's own `content/<slug>.md` (loaded via
//! `include_str!` by the brand crate — e.g. `neon/content`) and
//! pass through [`render`]. The result is a HTML string a caller drops into its
//! content slot.

use pulldown_cmark::{html, CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::components::code;

fn markdown_options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts
}

/// Render a `CommonMark` string to HTML.
///
/// Enables tables, strikethrough, footnotes, and task lists so authored
/// markdown matches what the workspace's own linter accepts.
#[must_use]
pub fn render(src: &str) -> String {
    let parser = Parser::new_ext(src, markdown_options());
    let mut out = String::new();
    html::push_html(&mut out, parser);
    code::decorate_copy_buttons(&out)
}

/// Replace each fenced/indented code block in a pulldown event stream with
/// server-side syntect-highlighted HTML (via [`code::highlight`]), so a Markdown
/// renderer that opts in emits coloured code with no client highlighter.
/// Non-code events pass through untouched. The fence info string's first token
/// is the language (`rust` from ```` ```rust ````); an indented or bare block
/// highlights as plain text.
///
/// This is the shared highlighting seam: the Catalog material renderer
/// (`portal::workshops::loader`) applies it, so a slide's code reads the same
/// on every deck face. The plain [`render`] path
/// deliberately does not — README/doc prose keeps its inert `language-…` fence
/// classes rather than gaining highlighting it never had.
#[must_use]
pub fn highlight_code_blocks(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut out = Vec::with_capacity(events.len());
    let mut lang: Option<String> = None;
    let mut code = String::new();
    for event in events {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                lang = Some(match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split([' ', ','])
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                });
                code.clear();
            }
            Event::Text(text) if lang.is_some() => code.push_str(&text),
            Event::End(TagEnd::CodeBlock) if lang.is_some() => {
                let lang = lang.take().unwrap_or_default();
                let html = code::with_copy_button(&code::highlight(&code, &lang));
                out.push(Event::Html(CowStr::from(html)));
                code.clear();
            }
            other => out.push(other),
        }
    }
    out
}

/// Extensions that make a markdown image reference a video rather than a
/// picture.
///
/// MP4 (H.264) is the only accepted format, deliberately. The rendered
/// `<video>` carries a single `src` rather than a list of `<source>`
/// children, so a second format would be an alternative to choose between
/// rather than a fallback — one more way to pick wrong, buying no reach we
/// do not already have. Requiring one format also keeps this list and the
/// asset pipeline's `content_type_for` trivially in step, which the
/// `every_renderable_video_extension_is_uploadable` guard pins: an
/// extension recognized here but not there would upload as nothing and
/// then 404 in every deployment.
///
/// Nothing in the workspace transcodes, so an author converts first.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4"];

/// True when a markdown image destination names a video file.
#[must_use]
pub fn is_video_src(dest: &str) -> bool {
    // Compare on the path only: a query or fragment must not defeat the
    // extension match, and the comparison is case-insensitive because a
    // camera or screen recorder may hand back `.MP4`.
    let path = dest.split(['?', '#']).next().unwrap_or(dest);
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        VIDEO_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// Replace each markdown image whose destination is a video with a
/// `<video>` element, so an author writes one syntax — `![caption](…)` —
/// for every kind of media and the renderer picks the right element.
///
/// Markdown has no video syntax, and the alternative is hand-written HTML
/// in content, which no linter checks and no asset gate follows. Reusing
/// the image syntax keeps a clip inside the seam that already resolves
/// asset URLs and the `assets verify` sweep that already walks `](img/…)`
/// references.
///
/// `<video>` has no `alt`, so the image's alt text becomes the accessible
/// name and the fallback content a browser shows when it cannot play the
/// file. Playback is `controls` and `preload="metadata"` — never autoplay:
/// a slide deck opens many slides at once, and a clip that starts itself
/// is both a bandwidth surprise and an accessibility failure.
#[must_use]
pub fn upgrade_video_images(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut out = Vec::with_capacity(events.len());
    // `Some(..)` while inside a video image, collecting its alt text; the
    // nested text events are the caption, not page content.
    let mut pending: Option<(String, String)> = None;
    for event in events {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) if is_video_src(&dest_url) => {
                pending = Some((dest_url.to_string(), String::new()));
            }
            Event::Text(text) | Event::Code(text) if pending.is_some() => {
                if let Some((_, alt)) = pending.as_mut() {
                    alt.push_str(&text);
                }
            }
            Event::End(TagEnd::Image) if pending.is_some() => {
                let (src, alt) = pending.take().unwrap_or_default();
                out.push(Event::Html(CowStr::from(video_html(&src, &alt))));
            }
            other => out.push(other),
        }
    }
    out
}

/// The `<video>` element for one resolved source and its caption. Both are
/// escaped: a destination and an alt string are author input, and this is
/// the one place they become raw HTML rather than pulldown-escaped text.
fn video_html(src: &str, alt: &str) -> String {
    let src = escape_attribute(src);
    let alt = escape_attribute(alt);
    format!(
        "<p><video src=\"{src}\" controls preload=\"metadata\" playsinline \
         aria-label=\"{alt}\">{alt}</video></p>"
    )
}

/// Escape the five characters that could break out of a double-quoted HTML
/// attribute or the surrounding element.
fn escape_attribute(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Like [`render`], but every link destination is passed through
/// `rewrite` first and every heading gets a slug `id` so in-page anchors
/// resolve. Used to serve repo-relative Markdown (a README, a doc) on the
/// web: a link written for a git reader (`docs/glossary/project.md`,
/// `templates/x/y.md`) is retargeted onto its site route, and a
/// same-page anchor (`#trademarks`) lands on the matching heading.
#[must_use]
pub fn render_with_link_rewrite(src: &str, rewrite: impl Fn(&str) -> String) -> String {
    let events: Vec<Event> = Parser::new_ext(src, markdown_options()).collect();
    let mut out_events: Vec<Event> = Vec::with_capacity(events.len());

    for i in 0..events.len() {
        match &events[i] {
            // Stamp a slug id on headings that don't already declare one.
            Event::Start(Tag::Heading {
                level,
                id: None,
                classes,
                attrs,
            }) => {
                let text = heading_text(&events[i + 1..]);
                out_events.push(Event::Start(Tag::Heading {
                    level: *level,
                    id: Some(slugify(&text).into()),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                }));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => out_events.push(Event::Start(Tag::Link {
                link_type: *link_type,
                dest_url: CowStr::from(rewrite(dest_url)),
                title: title.clone(),
                id: id.clone(),
            })),
            other => out_events.push(other.clone()),
        }
    }

    let mut out = String::new();
    html::push_html(&mut out, out_events.into_iter());
    code::decorate_copy_buttons(&out)
}

/// Concatenate the text of a heading from the events following its
/// `Start(Heading)` up to the matching `End`. `Code` spans count as text.
fn heading_text(rest: &[Event]) -> String {
    let mut text = String::new();
    for ev in rest {
        match ev {
            Event::End(TagEnd::Heading(_)) => break,
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            _ => {}
        }
    }
    text
}

/// GitHub-style heading slug: lowercase, drop punctuation, spaces → `-`,
/// keep existing hyphens and underscores.
#[must_use]
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        } else if c == '-' || c == '_' {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn renders_heading_and_emphasis() {
        let html = render("# Hello\n\nA *bold* claim.");
        assert!(html.contains("<h1>Hello</h1>"));
        assert!(html.contains("<em>bold</em>"));
    }

    #[test]
    fn highlight_code_blocks_colours_fenced_source_server_side() {
        // The opt-in helper turns a fenced block into syntect-coloured HTML —
        // inline styles in the markup — with no client-side highlighter class.
        use pulldown_cmark::Parser;
        let events: Vec<super::Event> = Parser::new("```rust\nlet x = 1;\n```\n").collect();
        let mut html = String::new();
        super::html::push_html(&mut html, super::highlight_code_blocks(events).into_iter());
        assert!(
            html.contains("style=\"color:"),
            "server-highlighted spans: {html}"
        );
        assert!(
            !html.contains("class=\"language-rust\""),
            "no hljs class: {html}"
        );
        assert!(
            html.contains("data-copy-code=\"true\""),
            "highlighted fence carries a copy button: {html}"
        );
        // The workshop renderer runs the HTML pass after highlighting. A fence
        // that already has a button must not gain a second one.
        let again = super::code::decorate_copy_buttons(&html);
        assert_eq!(
            again.matches("data-copy-code=\"true\"").count(),
            1,
            "decorating highlighted HTML must not nest a button: {again}"
        );
    }

    #[test]
    fn plain_render_puts_a_copy_button_on_each_fenced_block() {
        let html = render(
            "```bash\ncargo test\n```\n\nUse `cargo` inline.\n\n```rust\nfn main() {}\n```\n",
        );
        assert_eq!(
            html.matches("data-copy-code=\"true\"").count(),
            2,
            "one button per fence, not per inline code: {html}"
        );
        assert!(
            html.contains("<code>cargo</code>"),
            "inline code stays inline: {html}"
        );
        assert!(
            !html.contains("onclick"),
            "the button has no inline handler: {html}"
        );
    }

    #[test]
    fn plain_render_leaves_fenced_code_unhighlighted() {
        // The default prose path does not highlight — a fenced block keeps its
        // inert `language-…` class and its verbatim text (README/doc commands
        // stay matchable), no syntect spans.
        let html = render("```bash\ncargo run -p cli -- dev up\n```\n");
        assert!(
            html.contains("cargo run -p cli -- dev up"),
            "verbatim command: {html}"
        );
        assert!(
            !html.contains("style=\"color:"),
            "no syntect styling: {html}"
        );
    }

    #[test]
    fn renders_bulleted_list() {
        let html = render("- first\n- second\n");
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>first</li>"));
    }

    #[test]
    fn link_rewrite_retargets_only_relative_links() {
        use super::render_with_link_rewrite;
        let src = "[a](docs/x.md) and [b](https://example.com)";
        let html = render_with_link_rewrite(src, |d| {
            if d.starts_with("http") {
                d.to_string()
            } else {
                format!("/site/{d}")
            }
        });
        assert!(html.contains("href=\"/site/docs/x.md\""), "got: {html}");
        assert!(html.contains("href=\"https://example.com\""), "got: {html}");
    }

    #[test]
    fn link_rewrite_stamps_heading_ids_for_anchors() {
        use super::render_with_link_rewrite;
        // The in-page `#trademarks` anchor only resolves if the heading
        // carries a matching id.
        let html =
            render_with_link_rewrite("### Trademarks\n\n[x](#trademarks)", ToString::to_string);
        assert!(html.contains("id=\"trademarks\""), "got: {html}");
    }
}
