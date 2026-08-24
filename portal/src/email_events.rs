//! SendGrid Event Webhook receiver — the delivery-side half of the
//! email-audit picture.
//!
//! The request side (`portal::email`'s `LoggingEmail`) records one
//! `sent_emails` row per outbound attempt: that proves SendGrid
//! *accepted* the message (a 202), not that it was *delivered*. The
//! delivery side is this handler: SendGrid POSTs an array of
//! lifecycle events (`processed`, `delivered`, `open`, `click`,
//! `bounce`, `dropped`, `deferred`, `spam_report`, `unsubscribe`) to
//! a configured URL, and we land them as Snappy-compressed Parquet on
//! object storage (filesystem in dev, GCS in prod via the
//! [`cloud::StorageService`] trait) — the same Parquet-on-GCS shape
//! the `archives` snapshot writes, so BigQuery reads both through an
//! external table.
//!
//! Each event carries SendGrid's `sg_message_id` and the `custom_args`
//! we stamped at send time (`template_slug`, `person_id`, see
//! `workflows::email`), so the analytics join back to `sent_emails`
//! and `persons` needs no address parsing.
//!
//! ## Layout
//!
//! One POST → one Parquet object at
//! `email-events/data/dt=<YYYY-MM-DD>/<sha256(body)>.parquet`. The
//! `dt=` partition is Hive-style so a BigQuery external table can
//! prune by date; the date comes from the *first event's* timestamp,
//! and the filename is the SHA-256 of the raw body. Both are pure
//! functions of the payload, so SendGrid's at-least-once retries
//! (it re-POSTs the identical body on any non-2xx for 24h) overwrite
//! the same object instead of duplicating it — file-level idempotency
//! without a dedupe table. `sg_event_id` remains unique per event for
//! row-level dedupe at query time.
//!
//! ## Auth
//!
//! Two layers, mirroring [`crate::esignature_webhook`]:
//!
//! - **Path secret** (`/webhook/email-events/:secret`) — coarse "is this our
//!   endpoint", compared constant-time against
//!   [`crate::AppState::email_events_secret`] (from `SENDGRID_EVENTS_SECRET`).
//!   `None` in dev/tests accepts any token.
//! - **ECDSA/P-256 signature** over `timestamp || body` — the real gate.
//!   SendGrid's "Signed Event Webhook" signs each delivery with a private
//!   key and sends the base64 DER signature in
//!   `X-Twilio-Email-Event-Webhook-Signature` plus the signed timestamp in
//!   `X-Twilio-Email-Event-Webhook-Timestamp`. We verify against the
//!   issued public key in [`crate::AppState::sendgrid_events_public_key`]
//!   (from `SENDGRID_EVENTS_PUBLIC_KEY`), over the *raw* body, before any
//!   parse — so the bytes that become lake rows are the bytes SendGrid
//!   signed. When the key is configured the headers must be present and
//!   valid or the request is rejected (fail closed). `None` in dev/tests
//!   skips it; production gates both env vars at boot via
//!   `enforce_deployment_invariants`.

use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::inbound_email::constant_time_eq;
use crate::webhook_auth::verify_ecdsa_p256_der_b64;

/// SendGrid's signed-event headers. The signature is base64 DER ECDSA;
/// the timestamp is prepended to the raw body to form the signed payload.
const SIGNATURE_HEADER: &str = "x-twilio-email-event-webhook-signature";
const TIMESTAMP_HEADER: &str = "x-twilio-email-event-webhook-timestamp";

/// One SendGrid event, narrowed to the fields we model as columns.
/// Unknown fields are dropped here but preserved whole in
/// [`ParsedEvent::raw_json`], so a schema we don't yet map is never
/// lost. `template_slug` / `person_id` are the `custom_args` we
/// stamped at send time; SendGrid inlines custom args as top-level
/// keys on each event object.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SendGridEvent {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub sg_event_id: Option<String>,
    #[serde(default)]
    pub sg_message_id: Option<String>,
    #[serde(default)]
    pub template_slug: Option<String>,
    #[serde(default)]
    pub person_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// A parsed event plus its verbatim JSON, so the Parquet row can keep
/// the full payload in `raw_json` alongside the modeled columns.
#[derive(Debug, Clone)]
pub struct ParsedEvent {
    pub ev: SendGridEvent,
    pub raw_json: String,
}

/// Reasons the webhook cannot proceed.
#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("unauthorized: webhook secret mismatch")]
    Unauthorized,
    #[error("unauthorized: missing or invalid event-webhook signature")]
    UnauthorizedSignature,
    #[error("malformed event payload: {0}")]
    Payload(String),
    #[error("parquet encode failed: {0}")]
    Encode(String),
    #[error("storage write failed: {0}")]
    Storage(String),
}

impl IntoResponse for EventError {
    fn into_response(self) -> axum::response::Response {
        let code = match &self {
            Self::Unauthorized | Self::UnauthorizedSignature => StatusCode::UNAUTHORIZED,
            Self::Payload(_) => StatusCode::BAD_REQUEST,
            Self::Encode(_) | Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, self.to_string()).into_response()
    }
}

/// Verify SendGrid's Signed Event Webhook over the raw body. Returns
/// `Ok(())` when verification passes or when no public key is configured
/// (dev/test). The signed payload is the timestamp header value
/// concatenated with the raw body bytes — recomputed here over exactly
/// what we received, before any parse.
fn verify_signature(
    public_key_b64: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), EventError> {
    let Some(public_key_b64) = public_key_b64 else {
        return Ok(());
    };
    let signature = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!("email-events webhook: signature header absent");
            EventError::UnauthorizedSignature
        })?;
    let timestamp = headers
        .get(TIMESTAMP_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            tracing::warn!("email-events webhook: timestamp header absent");
            EventError::UnauthorizedSignature
        })?;
    let mut signed_payload = timestamp.as_bytes().to_vec();
    signed_payload.extend_from_slice(body);
    if !verify_ecdsa_p256_der_b64(public_key_b64, &signed_payload, signature) {
        tracing::warn!("email-events webhook: signature verification failed");
        return Err(EventError::UnauthorizedSignature);
    }
    Ok(())
}

/// Parse the POST body (a JSON array of event objects) into
/// [`ParsedEvent`]s, keeping each event's verbatim JSON.
pub fn parse_events(body: &[u8]) -> Result<Vec<ParsedEvent>, EventError> {
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(body).map_err(|e| EventError::Payload(e.to_string()))?;
    values
        .into_iter()
        .map(|v| {
            let raw_json = v.to_string();
            let ev: SendGridEvent =
                serde_json::from_value(v).map_err(|e| EventError::Payload(e.to_string()))?;
            Ok(ParsedEvent { ev, raw_json })
        })
        .collect()
}

/// Hive-style `dt=` partition for a batch, taken from the first
/// event's unix timestamp. Falls back to the epoch when no timestamp
/// is present (real SendGrid events always carry one).
#[must_use]
pub fn partition_date(events: &[ParsedEvent]) -> String {
    let secs = events.first().and_then(|e| e.ev.timestamp).unwrap_or(0);
    chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}

/// Storage key for a batch: a pure function of the raw body, so a
/// retried (byte-identical) delivery overwrites the same object.
#[must_use]
pub fn storage_key(body: &[u8], events: &[ParsedEvent]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(body);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        // Infallible: writing to a String never errors.
        let _ = write!(hex, "{b:02x}");
    }
    format!(
        "email-events/data/dt={}/{hex}.parquet",
        partition_date(events)
    )
}

fn str_col(events: &[ParsedEvent], f: impl Fn(&ParsedEvent) -> Option<String>) -> ArrayRef {
    Arc::new(StringArray::from(
        events.iter().map(f).collect::<Vec<Option<String>>>(),
    ))
}

/// Encode a batch of events as Snappy-compressed Parquet. All modeled
/// columns are nullable `Utf8` except `event_unix_ts` (`Int64`),
/// mirroring the `archives` snapshot's all-string-by-default scheme —
/// BigQuery reads these as `STRING`/`INT64` and queries `CAST`/
/// `TIMESTAMP_SECONDS` when they need a typed value.
pub fn encode_parquet(events: &[ParsedEvent]) -> anyhow::Result<Vec<u8>> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("sg_event_id", DataType::Utf8, true),
        Field::new("sg_message_id", DataType::Utf8, true),
        Field::new("event", DataType::Utf8, true),
        Field::new("email", DataType::Utf8, true),
        Field::new("template_slug", DataType::Utf8, true),
        Field::new("person_id", DataType::Utf8, true),
        Field::new("url", DataType::Utf8, true),
        Field::new("reason", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, true),
        Field::new("timestamp_utc", DataType::Utf8, true),
        Field::new("event_unix_ts", DataType::Int64, true),
        Field::new("raw_json", DataType::Utf8, true),
    ]));
    let columns: Vec<ArrayRef> = vec![
        str_col(events, |e| e.ev.sg_event_id.clone()),
        str_col(events, |e| e.ev.sg_message_id.clone()),
        str_col(events, |e| e.ev.event.clone()),
        str_col(events, |e| e.ev.email.clone()),
        str_col(events, |e| e.ev.template_slug.clone()),
        str_col(events, |e| e.ev.person_id.clone()),
        str_col(events, |e| e.ev.url.clone()),
        str_col(events, |e| e.ev.reason.clone()),
        str_col(events, |e| e.ev.status.clone()),
        str_col(events, |e| {
            e.ev.timestamp
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|dt| dt.to_rfc3339())
        }),
        Arc::new(Int64Array::from(
            events.iter().map(|e| e.ev.timestamp).collect::<Vec<_>>(),
        )),
        str_col(events, |e| Some(e.raw_json.clone())),
    ];
    let batch = RecordBatch::try_new(schema, columns)?;

    let mut buf: Vec<u8> = Vec::new();
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(&mut buf, batch.schema(), Some(props))?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(buf)
}

/// Webhook handler — verifies the path secret, parses the event
/// array, encodes one Parquet object, writes it to storage, returns
/// `204`. An empty batch is a no-op `204` (SendGrid occasionally
/// sends keepalive-shaped empty arrays).
pub async fn webhook(
    State(state): State<crate::AppState>,
    Path(provided): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, EventError> {
    // 1. Path secret (coarse). Constant-time when configured.
    if let Some(configured) = state.email_events_secret.as_deref() {
        if !constant_time_eq(&provided, configured) {
            tracing::warn!("email-events webhook: secret mismatch");
            return Err(EventError::Unauthorized);
        }
    }
    // 2. ECDSA signature over `timestamp || body` (the real gate). Verify
    //    BEFORE parsing so the digest covers the exact bytes we land.
    verify_signature(state.sendgrid_events_public_key.as_deref(), &headers, &body)?;
    let events = parse_events(&body)?;
    if events.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    let key = storage_key(&body, &events);
    let parquet = encode_parquet(&events).map_err(|e| EventError::Encode(e.to_string()))?;
    state
        .storage
        .put(&key, &parquet, "application/vnd.apache.parquet")
        .await
        .map_err(|e| EventError::Storage(e.to_string()))?;
    tracing::info!(events = events.len(), key = %key, "email-events: persisted batch");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::{
        encode_parquet, parse_events, partition_date, storage_key, verify_signature,
        SIGNATURE_HEADER, TIMESTAMP_HEADER,
    };
    use axum::http::HeaderMap;
    use base64::Engine;
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};
    use p256::pkcs8::EncodePublicKey;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    const SAMPLE: &[u8] = br#"[
        {"email":"a@example.com","timestamp":1716940800,"event":"delivered",
         "sg_event_id":"evt-1","sg_message_id":"msg-1","template_slug":"welcome",
         "person_id":"p-1"},
        {"email":"a@example.com","timestamp":1716940860,"event":"click",
         "sg_event_id":"evt-2","sg_message_id":"msg-1","url":"https://www.neonlaw.com",
         "template_slug":"welcome","person_id":"p-1"}
    ]"#;

    #[test]
    fn parses_event_array_and_custom_args() {
        let events = parse_events(SAMPLE).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].ev.event.as_deref(), Some("delivered"));
        assert_eq!(events[0].ev.template_slug.as_deref(), Some("welcome"));
        assert_eq!(events[0].ev.person_id.as_deref(), Some("p-1"));
        assert_eq!(events[1].ev.url.as_deref(), Some("https://www.neonlaw.com"));
        // raw_json preserves the whole object, including fields we
        // don't model as columns.
        assert!(events[1].raw_json.contains("\"event\":\"click\""));
    }

    #[test]
    fn rejects_non_array_payload() {
        assert!(parse_events(b"{\"not\":\"an array\"}").is_err());
    }

    #[test]
    fn partition_date_comes_from_first_event_timestamp() {
        let events = parse_events(SAMPLE).unwrap();
        // 1716940800 = 2024-05-29 00:00:00 UTC.
        assert_eq!(partition_date(&events), "2024-05-29");
    }

    #[test]
    fn storage_key_is_pure_function_of_body() {
        let events = parse_events(SAMPLE).unwrap();
        let k1 = storage_key(SAMPLE, &events);
        let k2 = storage_key(SAMPLE, &events);
        // Idempotent: a retried identical body yields the same key.
        assert_eq!(k1, k2);
        assert!(k1.starts_with("email-events/data/dt=2024-05-29/"));
        assert!(k1.ends_with(".parquet"));
    }

    /// Build a SendGrid-shaped signed request: returns the base64 DER
    /// public key plus a `HeaderMap` carrying the signature over
    /// `timestamp || body`, exactly as SendGrid's Signed Event Webhook
    /// presents it.
    fn signed_request(timestamp: &str, body: &[u8]) -> (String, HeaderMap) {
        let std = base64::engine::general_purpose::STANDARD;
        let sk = SigningKey::from_slice(&[0x42u8; 32]).expect("valid P-256 scalar");
        let pk = std.encode(
            sk.verifying_key()
                .to_public_key_der()
                .expect("encode SPKI")
                .as_bytes(),
        );
        let mut payload = timestamp.as_bytes().to_vec();
        payload.extend_from_slice(body);
        let sig: Signature = sk.sign(&payload);
        let mut headers = HeaderMap::new();
        headers.insert(
            SIGNATURE_HEADER,
            std.encode(sig.to_der().as_bytes()).parse().unwrap(),
        );
        headers.insert(TIMESTAMP_HEADER, timestamp.parse().unwrap());
        (pk, headers)
    }

    #[test]
    fn no_configured_key_skips_signature_check() {
        // dev/test posture: absent key → verification is a no-op pass.
        assert!(verify_signature(None, &HeaderMap::new(), SAMPLE).is_ok());
    }

    #[test]
    fn a_validly_signed_event_batch_verifies() {
        let (pk, headers) = signed_request("1716940800", SAMPLE);
        assert!(verify_signature(Some(&pk), &headers, SAMPLE).is_ok());
    }

    #[test]
    fn a_tampered_body_fails_signature_verification() {
        let (pk, headers) = signed_request("1716940800", SAMPLE);
        // Same signature + timestamp, but a body SendGrid never signed.
        assert!(verify_signature(Some(&pk), &headers, b"[{\"event\":\"forged\"}]").is_err());
    }

    #[test]
    fn a_missing_signature_header_is_rejected_when_a_key_is_configured() {
        let (pk, _headers) = signed_request("1716940800", SAMPLE);
        // Headers present but no signature: fail closed.
        let mut headers = HeaderMap::new();
        headers.insert(TIMESTAMP_HEADER, "1716940800".parse().unwrap());
        assert!(verify_signature(Some(&pk), &headers, SAMPLE).is_err());
    }

    #[test]
    fn parquet_round_trips_modeled_columns() {
        let events = parse_events(SAMPLE).unwrap();
        let bytes = encode_parquet(&events).unwrap();
        assert!(!bytes.is_empty());

        // Read back through a tempfile (matches the `archives`
        // parquet_io round-trip test; avoids an in-memory reader dep).
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, &bytes).unwrap();
        let file = std::fs::File::open(tmp.path()).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.collect::<Result<_, _>>().unwrap();
        let rows: usize = batches
            .iter()
            .map(arrow::array::RecordBatch::num_rows)
            .sum();
        assert_eq!(rows, 2);
        // 12 modeled columns.
        assert_eq!(batches[0].num_columns(), 12);
    }
}
