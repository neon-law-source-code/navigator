//! Sanitizers for `tracing` structured fields.
//!
//! The standing order in `telemetry/src/lib.rs` is identifiers and counts,
//! never content: telemetry leaves the firm's trust boundary and an email
//! address is client-identifying. These helpers are how a log field keeps the
//! operational signal it needed the address *for* without carrying the address
//! itself, and `cli/tests/no_address_in_telemetry.rs` recognizes them by name
//! as the sanitized shape.
//!
//! Every field name here is one the collector's redaction processor already
//! allows (`examples/deploy/k8s/observability/otel-collector.yaml`). A field
//! absent from that `allowed_keys` list is silently deleted on the export
//! path, so a sanitizer that produced a differently-named field would trade a
//! leak for a blank.
//!
//! # Why there is no payload digest here
//!
//! There is deliberately no helper that hashes a tool-call payload to record
//! *what* a caller passed. The agent-action authorization records carry no
//! arguments, argument keys, digest, or count — see `a2a::audit_authorization`
//! and `docs/aida-a2a-interaction.md` — and a digest is the wrong primitive to
//! reopen that with.
//!
//! A digest looks content-free and is not. An unsalted hash is deterministic by
//! construction, which is the property that makes it verifiable at all, and a
//! deterministic hash over a *knowable* input space is recovered by enumerating
//! that space. The arguments behind the confirmation gate are keyed by project
//! codes that appear in portal URLs and by firm addresses published on the
//! website, so the space is small and public: the reversal is cheap, and it is
//! cheapest exactly where the payload is most sensitive.
//!
//! Substring assertions do not detect this. A `sha256` renders as 64 characters
//! of `[0-9a-f]`, so "the digest does not contain the address" holds for every
//! possible input and says nothing about whether the address can be recovered.
//!
//! If the trail ever needs to name the call, the answer is an identifier into a
//! governed store rather than a hash of the payload. `a2a::PendingConfirmations`
//! is process-local and so cannot be that store. The export contract for these
//! records is tracked separately.

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

#[cfg(test)]
mod tests {
    use super::domain_of;

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
}
