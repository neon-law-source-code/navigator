//! Shared inline-link parsing for the relative-link rules
//! ([`crate::M057RelativeLinkResolves`] and
//! [`crate::M061WebPortableLink`]).
//!
//! Both rules read the destination of inline links (`[text](target)`)
//! and reason about whether the target is a repo-relative path. Image
//! embeds (`![alt](src)`) are deliberately skipped: image sources route
//! through the asset seam (`views::assets::asset_url`), which resolves
//! against the object store rather than the repo tree.

/// The on-disk file portion of a *relative* link target, or `None` when
/// the target is not a repo-relative path — an absolute URL, `mailto:`,
/// a site-absolute `/…`, a bare `#anchor`, a `{{template}}` placeholder,
/// or empty. The returned slice has any `#fragment`/`?query` stripped.
#[must_use]
pub(crate) fn relative_file_part(target: &str) -> Option<&str> {
    let t = target.trim();
    if t.is_empty()
        || t.starts_with('#')
        || t.starts_with('/')
        || t.contains("{{")
        || t.contains("://")
        || t.starts_with("mailto:")
        || t.starts_with("tel:")
    {
        return None;
    }
    let file_part = t.split(['#', '?']).next().unwrap_or("");
    if file_part.is_empty() {
        return None;
    }
    Some(file_part)
}

/// Inline-link targets (`[text](target)`) on a single line. Image
/// embeds (`![alt](src)`) are skipped. A trailing `"title"` inside the
/// parentheses is dropped so only the destination is returned.
#[must_use]
pub(crate) fn link_targets(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' {
            // Find the matching `)` of this `](...)`, honoring nesting.
            let mut depth = 1usize;
            let mut j = i + 2;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let inner_end = if depth == 0 { j - 1 } else { j };
            let inner = &line[i + 2..inner_end];
            if !is_image_link(bytes, i) {
                out.push(strip_title(inner).to_string());
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// Whether the `]` at `bracket` closes an image link — i.e. its matching
/// opening `[` is immediately preceded by `!`.
fn is_image_link(bytes: &[u8], bracket: usize) -> bool {
    let mut depth = 1usize;
    let mut k = bracket;
    while k > 0 {
        k -= 1;
        match bytes[k] {
            b']' => depth += 1,
            b'[' => {
                depth -= 1;
                if depth == 0 {
                    return k > 0 && bytes[k - 1] == b'!';
                }
            }
            _ => {}
        }
    }
    false
}

/// Drop a trailing `"title"` (or `'title'`) from a link destination,
/// returning just the URL/path portion.
fn strip_title(inner: &str) -> &str {
    match inner.find(['"', '\'']) {
        Some(idx) => inner[..idx].trim(),
        None => inner.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::{link_targets, relative_file_part};

    #[test]
    fn link_targets_reads_inline_links_and_skips_images() {
        assert_eq!(
            link_targets("see [a](one.md) and ![img](pic.png) then [b](two.md)"),
            vec!["one.md".to_string(), "two.md".to_string()]
        );
    }

    #[test]
    fn link_targets_drops_a_title_suffix() {
        assert_eq!(
            link_targets(r#"[x](path.md "A title")"#),
            vec!["path.md".to_string()]
        );
    }

    #[test]
    fn relative_file_part_strips_anchor_and_query() {
        assert_eq!(relative_file_part("g.md#frag"), Some("g.md"));
        assert_eq!(relative_file_part("g.md?v=1"), Some("g.md"));
    }

    #[test]
    fn relative_file_part_rejects_non_relative_targets() {
        for t in [
            "https://x.com",
            "mailto:x@y.com",
            "/docs/glossary",
            "#anchor",
            "{{url}}",
            "",
        ] {
            assert_eq!(relative_file_part(t), None, "{t} should be skipped");
        }
    }
}
