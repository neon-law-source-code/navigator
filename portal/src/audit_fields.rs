//! Sanitizers for `tracing` structured fields.
//!
//! The standing order in `telemetry/src/lib.rs` is identifiers and counts,
//! never content: telemetry leaves the firm's trust boundary and an email
//! address is client-identifying. These helpers are how a log field keeps the
//! operational signal it needed the address *for* without carrying the address
//! itself, and `cli/tests/no_address_in_telemetry.rs` recognizes them by name
//! as the sanitized shape.
//!
//! **Sanitizing a field and getting it past the collector are two different
//! questions, and only some of these names answer the second.** The redaction
//! processor is fail-closed
//! (`examples/deploy/k8s/observability/otel-collector.yaml`): a field absent
//! from its `allowed_keys` list is deleted on the export path. For the address
//! sanitizers — [`person_id_field`] and [`domain_of`] — the field name was
//! therefore chosen to be one that list already carries, because a
//! differently-named field would have traded a leak for a blank.
//!
//! [`argument_digest`] and [`argument_count`] are deliberately *not* held to
//! that, and the reason is worth stating so the omission is not read as an
//! oversight. They serve the agent-authorization records in `a2a.rs`, and the
//! whole of that record is already stripped on export: `allowed_keys` carries
//! no `tool`, no `decision`, no `task_id`, and no `event`, so its exported form
//! is `person_id` and `step` and is not an audit trail whatever these two
//! fields do. Their value is in the retained stdout and Cloud Logging copy,
//! which is where `a2a::audit_authorization` says the record of authority
//! lives. Making the *exported* record usable would take `tool`, `decision`,
//! and `task_id` as well — a decision about what the telemetry pipeline is
//! for, which belongs with the log-lake work and its deferred value-level and
//! log-body scrubs, not with a content fix.

/// Render a person's id for an audit field, or `none`.
///
/// The field is spelled `person_id` deliberately. These `target: "audit"`
/// records carry the only copy of an authorization decision, and the
/// collector drops any structured field absent from its `allowed_keys` list —
/// where `person_id` appears and `approver_person_id` does not. A more
/// descriptive name would be silently deleted on the export path, leaving the
/// decision with no actor again.
///
/// An id is an opaque UUID rather than client-identifying content, which is
/// what makes it loggable where the address it replaces is not.
#[must_use]
pub fn person_id_field(person: Option<&store::persons::Person>) -> String {
    person.map_or_else(|| "none".to_string(), |p| p.id.to_string())
}

/// The domain half of an address, for a log field that needed to know *where*
/// a message came from rather than *who* sent it.
///
/// The local part is the identifying half — `libra` in `libra@example.com`
/// names a person, while `example.com` names an organization and is the half
/// every operational question here actually turns on: did this token come from
/// the required workspace, is real support mail reaching us, does the DKIM
/// verdict match the sending domain. Returning only the domain keeps those
/// answers and drops the identity.
///
/// Named without the word "address" or "email" on purpose: the gate reads the
/// value expression, and a sanitizer that has to be spelled into an exclusion
/// list is one rename away from re-permitting the thing it excluded.
#[must_use]
pub fn domain_of(address: &str) -> &str {
    // A `From:` header arrives in RFC 5322 display-name form
    // (`Libra Example <libra@example.com>`), so read the last `@` and shed the
    // closing bracket. Everything before that `@` — including the display
    // name, which is a person's name — is dropped.
    match address.rsplit_once('@') {
        Some((_, domain)) => match domain.trim().trim_end_matches('>').trim() {
            // A trailing `@` carries no domain, and neither does a bare local
            // part. Both are malformed rather than a domain worth logging.
            "" => "unknown",
            domain => domain,
        },
        None => "unknown",
    }
}

/// A SHA-256 digest of a proposed tool-call payload, as lowercase hex.
///
/// **The decision this field records, and why.** The agent-authorization
/// records in `a2a.rs` used to log the payload whole
/// (`arguments = %pending_call.arguments`). It is a `serde_json::Value` taken
/// straight off the wire — `agent_router::RoutedCall` — and the tools that can
/// reach a confirmation gate are, by construction, the client-facing ones: the
/// gate is `mcp::tools::is_side_effecting` minus the CRM writes that never
/// leave the building. So the values on that field were disproportionately
/// recipient addresses, subject lines, and notation bodies. Measured against
/// this module's rule — identifiers and counts, never content — a whole
/// payload is content. A digest is an identifier and [`argument_count`] is a
/// count.
///
/// **Why not drop the payload with nothing in its place.** Those records are
/// the *entire* trail for agent authorization: `store/src` has no table behind
/// them, `a2a::audit_authorization` says so in as many words, and the pending
/// store cannot hold the payload on the trail's behalf either —
/// `a2a::PendingConfirmations` is in-process and best-effort by design, pruned
/// on every insert and read and gone on restart, so moving the payload there
/// would delete it rather than relocate it. An approval record that cannot say
/// *what* was approved invites reliance it cannot support.
///
/// **What the digest buys.** Two records describe the same proposed call
/// exactly when their digests match, so a proposal ties to its authorization
/// on content as well as on `task_id`; and a payload substituted after the fact
/// cannot pass as the approved one.
///
/// **Determinism.** `serde_json` is built without `preserve_order` (see the
/// workspace `Cargo.toml`), so a `Value` object is a `BTreeMap` and renders its
/// keys sorted. Two structurally equal payloads therefore serialize to
/// identical bytes and hash identically, whatever order they arrived in. The
/// hash goes through `store::assets::sha256_hex` rather than a local `Sha256`
/// so there is one digest spelling in the workspace, for the same reason
/// `store::documents::sha256_hex` delegates to it.
///
/// **What this costs, recorded so a later reader does not restore the payload
/// believing the field was dropped by accident.** A digest is *not
/// human-readable during an incident*. A responder cannot read off which
/// address was emailed; they must obtain the artifact the call produced, and
/// the field then only confirms that it matches. That is the accepted price of
/// keeping client content out of a log retained for ten years.
///
/// **Deliberately not logged: the argument key names.** They are schema today,
/// but nothing in the tool contract stops a payload from being keyed by client
/// data, and the rule admits identifiers and counts — not schema fragments.
#[must_use]
pub fn argument_digest(arguments: &serde_json::Value) -> String {
    store::assets::sha256_hex(arguments.to_string().as_bytes())
}

/// How many top-level arguments a proposed call carried.
///
/// The countable half of [`argument_digest`]: it satisfies the rule on its own
/// terms, and it is the one thing a reader can act on without fetching the
/// artifact — a notation approved with two arguments is visibly not the
/// four-argument call someone remembers approving.
///
/// A payload that is not a JSON object has no named arguments and counts zero.
/// That case is reachable: `a2a::resolve_arguments` hands back a `data` Part
/// verbatim, and an A2A client may put an array or a scalar there.
#[must_use]
pub fn argument_count(arguments: &serde_json::Value) -> usize {
    arguments.as_object().map_or(0, serde_json::Map::len)
}

#[cfg(test)]
mod tests {
    use super::{argument_count, argument_digest, domain_of};
    use serde_json::json;

    #[test]
    fn keeps_the_domain_and_drops_the_local_part() {
        assert_eq!(domain_of("libra@example.com"), "example.com");
        assert_eq!(domain_of("someone@neonlaw.com"), "neonlaw.com");
    }

    /// A display-name form still yields the domain, and still drops the name.
    #[test]
    fn reads_the_last_at_sign() {
        assert_eq!(
            domain_of("Libra Example <libra@example.com>"),
            "example.com"
        );
    }

    #[test]
    fn reports_unknown_rather_than_leaking_a_malformed_value() {
        assert_eq!(domain_of(""), "unknown");
        assert_eq!(domain_of("not-an-address"), "unknown");
        assert_eq!(domain_of("trailing@"), "unknown");
    }

    /// Determinism. The digest's whole claim is "two records describe the same
    /// proposed call exactly when their digests match", and that claim fails if
    /// key order changes the bytes. It does not, because `serde_json` is built
    /// without `preserve_order` and so renders object keys sorted — but that is
    /// a *dependency feature*, invisible at the call site and switchable by an
    /// unrelated crate enabling it. This test is what makes such a switch fail
    /// loudly here rather than silently break the trail's substitution check.
    #[test]
    fn structurally_equal_payloads_hash_equal_whatever_the_key_order() {
        let one = json!({ "subject": "Welcome", "to": "a@b.test", "cc": null });
        let other = json!({ "cc": null, "to": "a@b.test", "subject": "Welcome" });
        assert_eq!(
            argument_digest(&one),
            argument_digest(&other),
            "key order must not change the digest of the same call"
        );
        let digest = argument_digest(&one);
        assert_eq!(
            digest.len(),
            64,
            "a sha-256 renders as 64 lowercase hex characters"
        );
        assert!(
            digest.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "the digest must be lowercase hex so two copies compare as strings"
        );
    }

    /// Sensitivity — the other half of the same claim. A payload substituted
    /// after the approval must not be able to pass as the approved one, so a
    /// changed *value* under an unchanged key must change the digest. This is
    /// the property that makes the digest worth logging at all instead of
    /// nothing.
    #[test]
    fn substituting_an_argument_value_changes_the_digest() {
        let approved = json!({ "person_id": "11111111-1111-1111-1111-111111111111" });
        let substituted = json!({ "person_id": "22222222-2222-2222-2222-222222222222" });
        assert_ne!(
            argument_digest(&approved),
            argument_digest(&substituted),
            "a different recipient must not share the approved call's digest"
        );
        // Adding an argument is a substitution too.
        assert_ne!(
            argument_digest(&approved),
            argument_digest(&json!({
                "person_id": "11111111-1111-1111-1111-111111111111",
                "bcc": "somebody@else.test",
            })),
            "an extra argument must not share the approved call's digest"
        );
    }

    /// The digest is of the payload alone, so a reader holding a payload can
    /// recompute it without knowing anything about how the record was framed.
    #[test]
    fn the_digest_is_the_sha256_of_the_payloads_json_encoding() {
        let payload = json!({ "person_id": "11111111-1111-1111-1111-111111111111" });
        assert_eq!(
            argument_digest(&payload),
            store::assets::sha256_hex(
                r#"{"person_id":"11111111-1111-1111-1111-111111111111"}"#.as_bytes()
            ),
            "the digest must be recomputable from the payload by hand"
        );
    }

    /// `argument_count` counts top-level arguments, and answers rather than
    /// panics on the payload shapes the A2A contract actually admits:
    /// `a2a::resolve_arguments` hands back a `data` Part verbatim, so an array
    /// or a scalar can arrive where an object was expected.
    #[test]
    fn argument_count_counts_top_level_fields_and_zero_for_a_non_object() {
        assert_eq!(argument_count(&json!({ "a": 1, "b": 2, "c": 3 })), 3);
        assert_eq!(
            argument_count(&json!({ "a": { "b": 1, "c": 2 } })),
            1,
            "nesting is not a top-level argument, and counting it would leak shape"
        );
        assert_eq!(argument_count(&json!({})), 0);
        assert_eq!(
            argument_count(&json!([1, 2, 3])),
            0,
            "an array names no arguments"
        );
        assert_eq!(argument_count(&serde_json::Value::Null), 0);
    }

    /// The count is a count, not a lossy digest: it must not vary with the
    /// *content* of the values. Guards the one way this field could drift into
    /// content, which is somebody deciding it would be more useful as the
    /// payload's byte length.
    #[test]
    fn argument_count_does_not_vary_with_the_values() {
        assert_eq!(
            argument_count(&json!({ "to": "a@b.test" })),
            argument_count(&json!({ "to": "a-very-much-longer-address@example.test" })),
            "the count describes how many arguments there were, never how big"
        );
    }
}
