//! Vendored government forms — the metadata registry behind
//! `templates/notations/forms/`.
//!
//! The blank PDF bytes live **only** in the public assets bucket, at
//! each form's `object_path`. The repository keeps the diffable text:
//! the sibling markdown template (the catalog card), the re-authored
//! blank's `.fields` manifest — its `/T` names *are* question paths, so
//! [`fill_values`] fills with no map at all — and a `.sha256` pin of the
//! canonical blank. A `<code>.fields.toml` map is the one-time input
//! `navigator forms re-author` consumes (see [`fieldmap`]); it is
//! deleted once the blank is re-authored. The fill path pulls the blank
//! through `cloud::StorageService` and must verify it against the pin
//! before filling — a mismatch or a missing object is a loud failure,
//! never a fallback.

pub mod fieldmap;
pub mod reauthor;

pub use fieldmap::{parse_field_map, FieldMap, FieldMapError, FieldRule};
pub use reauthor::{manifest, resolve_reauthored, ReauthorPlanError, UNMAPPED_PREFIX};

/// Fill values for one form through its re-authored `.fields` manifest —
/// each `/T` name *is* its data path. `Ok(None)` when the form carries no
/// manifest — a reference document nothing fills.
///
/// # Errors
///
/// [`FieldMapError`] from manifest resolution.
pub fn fill_values(
    form_code: &str,
    answers: &std::collections::BTreeMap<String, String>,
) -> Result<Option<std::collections::BTreeMap<String, String>>, FieldMapError> {
    match reauthor::manifest(form_code) {
        Some(names) => {
            let names: Vec<String> = names.into_iter().map(ToString::to_string).collect();
            Ok(Some(reauthor::resolve_reauthored_with_title_defaults(
                &names,
                answers,
                title_defaults(form_code),
            )?))
        }
        None => Ok(None),
    }
}

fn title_defaults(form_code: &str) -> &'static [(&'static str, &'static str)] {
    match form_code {
        "nv__business_trust_formation" => &[("people__trustees", "Trustee")],
        _ => &[],
    }
}

/// Metadata for one vendored government form. Carries no bytes: the
/// blank itself lives in the assets bucket at
/// [`object_path`](Self::object_path), pinned by
/// [`pinned_sha256`](Self::pinned_sha256).
#[derive(Debug, Clone)]
pub struct FormMeta {
    /// Stable form/template code. For forms, this is jurisdiction-first:
    /// `nv__llc_formation`, `us__form_990`, etc.
    pub code: &'static str,
    /// Jurisdiction code from `store/seeds/Jurisdiction.yaml`.
    pub jurisdiction: &'static str,
    /// Human title from the sibling markdown template.
    pub title: &'static str,
    /// Canonical government page where the blank can be obtained.
    pub origin_url: &'static str,
    /// Path of the blank in the public assets bucket. With `templates/`
    /// prepended it is also the repo path of the untracked working copy
    /// `navigator forms sync` uploads.
    pub object_path: &'static str,
    /// Raw contents of the sibling `.sha256` pin file — the sha-256 hex
    /// digest of the canonical blank. Public so a test harness can stage
    /// a synthetic blank under its own pin; production code reads
    /// [`pinned_sha256`](Self::pinned_sha256).
    pub sha256_pin: &'static str,
}

impl FormMeta {
    /// The pinned sha-256 hex digest of the canonical blank.
    #[must_use]
    pub fn pinned_sha256(&self) -> &'static str {
        self.sha256_pin.trim()
    }

    /// Verify `bytes` against this form's pinned digest.
    ///
    /// # Errors
    ///
    /// [`IntegrityError`] when the digest does not match — the bytes are
    /// not the blank the repository pinned and must not be filled.
    pub fn verify(&self, bytes: &[u8]) -> Result<(), IntegrityError> {
        verify_sha256(self.pinned_sha256(), bytes)
    }
}

/// Errors loading the bundled registry.
#[derive(Debug, thiserror::Error)]
pub enum FormsError {
    #[error("forms registry unavailable")]
    Unavailable,
}

/// A blank's bytes do not match the repository's `.sha256` pin — the
/// bucket object was re-vendored (or tampered with) without updating
/// the pin. The fill path must stop here, loudly.
#[derive(Debug, thiserror::Error)]
#[error("blank form bytes do not match the pinned sha256 (pinned {pinned}, got {actual})")]
pub struct IntegrityError {
    pub pinned: String,
    pub actual: String,
}

/// The lowercase sha-256 hex digest of `bytes` — the encoding the
/// `.sha256` pin files carry.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Verify `bytes` against a pinned sha-256 hex digest.
///
/// # Errors
///
/// [`IntegrityError`] when the digest does not match.
pub fn verify_sha256(pinned: &str, bytes: &[u8]) -> Result<(), IntegrityError> {
    let actual = sha256_hex(bytes);
    if actual == pinned.trim() {
        Ok(())
    } else {
        Err(IntegrityError {
            pinned: pinned.trim().to_string(),
            actual,
        })
    }
}

const NV_SOS_FORMS_URL: &str =
    "https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms";
const USCIS_N400_URL: &str = "https://www.uscis.gov/n-400";

const BUNDLED: &[FormMeta] = &[
    FormMeta {
        code: "us__naturalization",
        jurisdiction: "US",
        title: "Application for Naturalization -- Form N-400",
        origin_url: USCIS_N400_URL,
        object_path: "forms/united_states/federal/uscis/us__naturalization.pdf",
        sha256_pin: include_str!(
            "../../templates/notations/forms/united_states/federal/uscis/us__naturalization.sha256"
        ),
    },
    FormMeta {
        code: "nv__llc_formation",
        jurisdiction: "NV",
        title: "Nevada LLC Formation",
        origin_url: NV_SOS_FORMS_URL,
        object_path: "forms/united_states/nevada/state/nv__llc_formation.pdf",
        sha256_pin: include_str!(
            "../../templates/notations/forms/united_states/nevada/state/nv__llc_formation.sha256"
        ),
    },
    FormMeta {
        code: "nv__profit_corp_formation",
        jurisdiction: "NV",
        title: "Nevada Profit Corporation Formation",
        origin_url: NV_SOS_FORMS_URL,
        object_path: "forms/united_states/nevada/state/nv__profit_corp_formation.pdf",
        sha256_pin: include_str!(
            "../../templates/notations/forms/united_states/nevada/state/nv__profit_corp_formation.sha256"
        ),
    },
    FormMeta {
        code: "nv__business_trust_formation",
        jurisdiction: "NV",
        title: "Nevada Business Trust Formation",
        origin_url: NV_SOS_FORMS_URL,
        object_path: "forms/united_states/nevada/state/nv__business_trust_formation.pdf",
        sha256_pin: include_str!(
            "../../templates/notations/forms/united_states/nevada/state/nv__business_trust_formation.sha256"
        ),
    },
];

/// Return the bundled form registry.
///
/// # Errors
///
/// This currently cannot fail; the `Result` keeps the public seam stable
/// for callers that already propagate registry errors.
pub fn registry() -> Result<Vec<FormMeta>, FormsError> {
    Ok(BUNDLED.to_vec())
}

/// Look up one vendored form by its stable `code`.
///
/// # Errors
///
/// Propagates [`registry`] errors; `Ok(None)` when the code is unknown.
pub fn get(code: &str) -> Result<Option<FormMeta>, FormsError> {
    Ok(registry()?.into_iter().find(|f| f.code == code))
}

#[cfg(test)]
mod tests {
    use super::{fill_values, get, registry, sha256_hex, verify_sha256};

    #[test]
    fn fill_values_is_none_for_a_form_with_no_manifest() {
        let answers = std::collections::BTreeMap::new();
        assert!(fill_values("not_a_vendored_form", &answers)
            .expect("no error")
            .is_none());
    }

    #[test]
    fn registry_pins_every_blank() {
        let forms = registry().expect("registry loads");
        assert_eq!(forms.len(), 4);
        for form in &forms {
            let pin = form.pinned_sha256();
            assert_eq!(pin.len(), 64, "{}: pin is not a sha-256 digest", form.code);
            assert!(
                pin.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{}: pin is not lowercase hex",
                form.code
            );
            assert!(std::path::Path::new(form.object_path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")));
            assert!(form.object_path.starts_with("forms/united_states/"));
        }
    }

    #[test]
    fn get_finds_known_and_misses_unknown() {
        assert!(get("nv__llc_formation").expect("registry loads").is_some());
        assert!(get("nv__annual_list").expect("registry loads").is_none());
    }

    #[test]
    fn sha_verification_accepts_the_pinned_bytes_and_rejects_others() {
        let bytes = b"%PDF-1.5 canonical blank";
        let pin = sha256_hex(bytes);
        verify_sha256(&pin, bytes).expect("matching bytes verify");
        // A trailing newline in the pin file is tolerated.
        verify_sha256(&format!("{pin}\n"), bytes).expect("pin file newline is trimmed");
        let err = verify_sha256(&pin, b"%PDF-1.5 re-vendored blank").unwrap_err();
        assert_eq!(err.pinned, pin);
        assert_ne!(err.actual, err.pinned);
    }
}
