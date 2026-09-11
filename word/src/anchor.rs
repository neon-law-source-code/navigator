//! Stable source anchors for imported Word blocks.

/// Build the deterministic fallback anchor used when a Word paragraph does
/// not carry an explicit `w14:paraId`.
#[must_use]
pub fn block_anchor(part_uri: &str, kind: &str, ordinal: usize) -> String {
    format!("{part_uri}:{kind}:{ordinal}")
}

/// Normalize an adapter-provided paragraph id into the same anchor namespace
/// as fallback anchors.
#[must_use]
pub fn paragraph_anchor(part_uri: &str, paragraph_id: Option<&str>, ordinal: usize) -> String {
    paragraph_id.map_or_else(
        || block_anchor(part_uri, "paragraph", ordinal),
        |id| format!("{part_uri}:paragraph:{id}"),
    )
}
