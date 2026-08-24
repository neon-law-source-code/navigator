//! Deployment-owned branding loaded from a read-only mounted bundle.
//!
//! The bundle is deliberately separate from object storage: it contains only
//! identity metadata and static brand files supplied by the deployment
//! operator. Client documents, generated PDFs, form blanks, exports, and LFS
//! objects stay in their respective storage lanes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Opt-in switch for custom (white-label) branding. Its value is the path
/// to the mounted brand bundle directory. Unset → the built-in Neon Law
/// default identity; set → the deployment asserts a custom brand and must
/// supply a valid bundle at that path (fail closed within custom mode).
pub const CUSTOM_BRANDING_ENV: &str = "NAVIGATOR_CUSTOM_BRANDING";
/// The only manifest filename in a bundle.
pub const MANIFEST_FILE: &str = "navigator.yaml";
/// Schema version accepted by this binary.
pub const SCHEMA_VERSION: u32 = 1;

/// Fully validated deployment branding. It is immutable and passed to the
/// application at construction; loading it never changes process variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandBundle {
    pub manifest: BrandManifest,
    pub directory: PathBuf,
}

impl BrandBundle {
    /// Resolve the deployment's brand bundle from the environment. Branding
    /// is Neon Law by default: [`CUSTOM_BRANDING_ENV`] unset → `Ok(None)`,
    /// and the caller renders the built-in identity. Setting it opts into
    /// custom branding — its value is the bundle path, which must load and
    /// validate (fail closed within custom mode), so a deployment that means
    /// to rebrand can never silently fall back to Neon Law.
    pub fn from_env() -> Result<Option<Self>, BundleError> {
        Self::from_env_with(|key| std::env::var(key).ok())
    }

    /// [`from_env`](Self::from_env) with the environment injected, so the
    /// resolution is unit-tested without mutating process variables (the
    /// getter-closure pattern shared with `ship`'s substitution resolver).
    pub fn from_env_with<F>(get: F) -> Result<Option<Self>, BundleError>
    where
        F: Fn(&str) -> Option<String>,
    {
        match get(CUSTOM_BRANDING_ENV).filter(|dir| !dir.trim().is_empty()) {
            Some(dir) => Self::load(dir).map(Some),
            None => Ok(None),
        }
    }

    /// Load and validate a mounted bundle.
    pub fn load(directory: impl AsRef<Path>) -> Result<Self, BundleError> {
        let directory = directory.as_ref().to_path_buf();
        let manifest_path = directory.join(MANIFEST_FILE);
        let mut diagnostics = Vec::new();
        let raw = match fs::read_to_string(&manifest_path) {
            Ok(raw) => raw,
            Err(error) => {
                return Err(BundleError(vec![format!(
                    "reading {}: {error}",
                    manifest_path.display()
                )]));
            }
        };
        let manifest: BrandManifest = match serde_yaml::from_str(&raw) {
            Ok(manifest) => manifest,
            Err(error) => {
                return Err(BundleError(vec![format!(
                    "parsing {}: {error}",
                    manifest_path.display()
                )]))
            }
        };
        validate(&manifest, &directory, &mut diagnostics);
        if diagnostics.is_empty() {
            Ok(Self {
                manifest,
                directory,
            })
        } else {
            Err(BundleError(diagnostics))
        }
    }
}

/// Validate a parsed manifest against the directory that owns its relative
/// files. The CLI uses the same validator before building a bundle.
pub fn validate_manifest(
    manifest: &BrandManifest,
    directory: impl AsRef<Path>,
) -> Result<(), BundleError> {
    let mut diagnostics = Vec::new();
    validate(manifest, directory.as_ref(), &mut diagnostics);
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(BundleError(diagnostics))
    }
}

/// Versioned on-disk bundle manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrandManifest {
    pub version: u32,
    #[serde(default)]
    pub brand: Brand,
    #[serde(default)]
    pub portal_only: bool,
    #[serde(default)]
    pub assets: Assets,
}

/// Identity fields owned by the deployment operator.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Brand {
    pub firm: Option<String>,
    /// The legal person named in the footer's copyright line. Configurable so
    /// a rename is a manifest edit, not a code change; unset keeps today's
    /// value.
    pub firm_legal_entity: Option<String>,
    /// The firm's whole inbound address, when it is not `support@` on
    /// [`Brand::support_domain`]. Wins over `support_domain` when both are set.
    pub support_email: Option<String>,
    /// The host the firm's support address is built on — unset `support_email`
    /// and the address is `support@{support_domain}`. Distinct from
    /// [`Brand::primary_domain`], which is the infrastructure apex.
    pub support_domain: Option<String>,
    /// The firm's published voice line, shown on `/contact`. Unset keeps the
    /// built-in number — a white-label deployment sets its own.
    pub firm_phone: Option<String>,
    /// Every office the firm publishes in the public footer. Each entry is the
    /// state it sits in and its street address. Empty (the default) keeps the
    /// built-in offices. Distinct from [`Brand::firm_address`], which is the
    /// single registered address the letterhead carries.
    #[serde(default)]
    pub firm_offices: Vec<OfficeEntry>,
    /// The firm's attorneys and the set of bar licenses each holds, rendered in
    /// the public footer. This is the footer's only bar disclosure, and it
    /// names who holds each licence. Empty (the default) keeps the built-in
    /// attorneys.
    #[serde(default)]
    pub firm_attorneys: Vec<AttorneyEntry>,
    pub firm_address: Option<String>,
    pub base_url: Option<String>,
    pub primary_domain: Option<String>,
    pub consultation_url: Option<String>,
    pub terms_url: Option<String>,
    pub privacy_url: Option<String>,
}

/// One manifest attorney entry: the attorney's full legal name and every bar
/// license they hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttorneyEntry {
    pub name: String,
    #[serde(default)]
    pub licenses: Vec<BarLicenseEntry>,
}

/// One manifest bar-license entry: the jurisdiction, the number it publishes
/// the attorney under, and the public record that number can be checked in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BarLicenseEntry {
    pub jurisdiction: String,
    pub number: String,
    pub license_url: String,
}

/// One manifest office entry: the state the office sits in and the street
/// address the public footer renders under it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OfficeEntry {
    pub state: String,
    pub address: String,
    /// A qualification published under the address — e.g. `bar admission
    /// pending` for a jurisdiction no attorney is admitted in yet. Unset
    /// publishes the address on its own. Mirrors
    /// [`crate::brand::FirmOffice::note`]; set it deliberately, because it is a
    /// regulated statement about where the firm may practise.
    pub note: Option<String>,
}

/// Static brand files relative to the bundle root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assets {
    pub firm_logo: Option<PathBuf>,
    pub firm_logo_raster: Option<PathBuf>,
    /// Additional deployment-owned public files, keyed by their safe relative
    /// public path below `/public/brand/static/`.
    #[serde(default)]
    pub static_files: BTreeMap<String, PathBuf>,
}

impl Assets {
    #[must_use]
    pub fn entries(&self) -> Vec<(String, &PathBuf)> {
        let mut entries = Vec::new();
        for (field, path) in [
            ("assets.firm_logo", self.firm_logo.as_ref()),
            ("assets.firm_logo_raster", self.firm_logo_raster.as_ref()),
        ] {
            if let Some(path) = path {
                entries.push((field.to_string(), path));
            }
        }
        entries.extend(
            self.static_files.iter().map(|(public_path, source)| {
                (format!("assets.static_files.{public_path}"), source)
            }),
        );
        entries
    }
}

/// All validation failures collected before the server accepts traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleError(pub Vec<String>);

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid Navigator brand bundle:")?;
        for diagnostic in &self.0 {
            write!(f, "\n- {diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BundleError {}

fn validate(manifest: &BrandManifest, directory: &Path, diagnostics: &mut Vec<String>) {
    if manifest.version != SCHEMA_VERSION {
        diagnostics.push(format!(
            "version must be {SCHEMA_VERSION}, got {}",
            manifest.version
        ));
    }
    if manifest.portal_only && manifest.brand.terms_url.as_deref().unwrap_or("").is_empty() {
        diagnostics.push(
            "portal_only is true but brand.terms_url is empty; a portal-only deployment must link to its own hosted terms of use".into(),
        );
    }
    for (field, value) in [
        ("brand.firm", &manifest.brand.firm),
        ("brand.firm_legal_entity", &manifest.brand.firm_legal_entity),
        ("brand.firm_phone", &manifest.brand.firm_phone),
        ("brand.firm_address", &manifest.brand.firm_address),
    ] {
        if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
            diagnostics.push(format!("{field} must be omitted or non-empty"));
        }
    }
    for (field, value) in [("brand.support_email", &manifest.brand.support_email)] {
        if let Some(value) = value {
            if value.trim() != value
                || value.chars().any(char::is_whitespace)
                || !value.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
                })
            {
                diagnostics.push(format!("{field} must be a valid email address"));
            }
        }
    }
    // `support_domain` becomes the right-hand side of `support@{HOST}`, so a
    // malformed value would ship a malformed address rather than fail here.
    if let Some(domain) = &manifest.brand.support_domain {
        if domain.trim() != domain
            || domain.chars().any(char::is_whitespace)
            || domain.contains('@')
            || !domain.contains('.')
            || domain.starts_with('.')
            || domain.ends_with('.')
        {
            diagnostics
                .push("brand.support_domain must be a bare hostname such as example.com".into());
        }
    }
    for (field, value) in [
        ("brand.base_url", &manifest.brand.base_url),
        ("brand.consultation_url", &manifest.brand.consultation_url),
        ("brand.terms_url", &manifest.brand.terms_url),
        ("brand.privacy_url", &manifest.brand.privacy_url),
    ] {
        if let Some(value) = value {
            let valid = url::Url::parse(value)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some());
            if !valid {
                diagnostics.push(format!("{field} must be an absolute http(s) URL"));
            }
        }
    }
    validate_footer_contact(&manifest.brand, diagnostics);
    if let Some(domain) = &manifest.brand.primary_domain {
        if domain.is_empty()
            || domain.trim() != domain
            || domain.contains("//")
            || domain.contains('/')
            || domain.chars().any(char::is_whitespace)
        {
            diagnostics
                .push("brand.primary_domain must be a bare DNS name without scheme or path".into());
        }
    }
    validate_asset_paths(&manifest.assets, directory, diagnostics);
}

/// Validate the footer's contact block: the published offices and the
/// per-attorney bar licences.
///
/// A published bar number is a verifiable claim about who may practise law, so
/// it must be present and must link to a record that can actually be opened —
/// a blank number, a nameless attorney, or a dead link is a false statement
/// about a licence, not a cosmetic defect.
fn validate_footer_contact(brand: &Brand, diagnostics: &mut Vec<String>) {
    for (index, office) in brand.firm_offices.iter().enumerate() {
        for (field, value) in [("state", &office.state), ("address", &office.address)] {
            if value.trim().is_empty() {
                diagnostics.push(format!(
                    "brand.firm_offices[{index}].{field} must be non-empty"
                ));
            }
        }
        // A blank `note` is worse than an absent one: it renders as an empty
        // qualification under a published address. Omit the key instead.
        if office.note.as_ref().is_some_and(|n| n.trim().is_empty()) {
            diagnostics.push(format!(
                "brand.firm_offices[{index}].note must be non-empty when set"
            ));
        }
    }
    for (index, attorney) in brand.firm_attorneys.iter().enumerate() {
        if attorney.name.trim().is_empty() {
            diagnostics.push(format!(
                "brand.firm_attorneys[{index}].name must be non-empty"
            ));
        }
        if attorney.licenses.is_empty() {
            diagnostics.push(format!(
                "brand.firm_attorneys[{index}].licenses must list at least one bar licence"
            ));
        }
        for (license_index, license) in attorney.licenses.iter().enumerate() {
            let at = format!("brand.firm_attorneys[{index}].licenses[{license_index}]");
            for (field, value) in [
                ("jurisdiction", &license.jurisdiction),
                ("number", &license.number),
            ] {
                if value.trim().is_empty() {
                    diagnostics.push(format!("{at}.{field} must be non-empty"));
                }
            }
            let valid = url::Url::parse(&license.license_url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some());
            if !valid {
                diagnostics.push(format!("{at}.license_url must be an absolute http(s) URL"));
            }
        }
    }
}

fn validate_asset_paths(assets: &Assets, directory: &Path, diagnostics: &mut Vec<String>) {
    for public_path in assets.static_files.keys() {
        let path = Path::new(public_path);
        if public_path.is_empty()
            || path.is_absolute()
            || public_path.contains(['{', '}', '*', '\\'])
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::CurDir
                        | Component::ParentDir
                        | Component::RootDir
                        | Component::Prefix(_)
                )
            })
        {
            diagnostics.push(format!(
                "assets.static_files key must be a safe relative public path: {public_path}"
            ));
        }
    }
    for (field, path) in assets.entries() {
        if path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            diagnostics.push(format!(
                "{field} must be a safe relative bundle path: {}",
                path.display()
            ));
            continue;
        }
        let resolved = directory.join(path);
        if !resolved.is_file() {
            diagnostics.push(format!("{field} is missing: {}", resolved.display()));
            continue;
        }
        let root = directory.canonicalize();
        let file = resolved.canonicalize();
        if let (Ok(root), Ok(file)) = (root, file) {
            if !file.starts_with(&root) {
                diagnostics.push(format!(
                    "{field} resolves outside the bundle root: {}",
                    path.display()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_bundle(body: &str) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(MANIFEST_FILE), body).unwrap();
        dir
    }

    #[test]
    fn unset_custom_branding_uses_neon_law_default() {
        // No NAVIGATOR_CUSTOM_BRANDING → no bundle; the caller renders the
        // built-in Neon Law identity.
        assert!(BrandBundle::from_env_with(|_| None).unwrap().is_none());
        assert!(BrandBundle::from_env_with(|_| Some("   ".into()))
            .unwrap()
            .is_none());
    }

    #[test]
    fn from_env_reads_the_process_environment() {
        // Exercises the real `from_env` wrapper over `std::env`. No test sets
        // NAVIGATOR_CUSTOM_BRANDING, so the process default is the Neon Law
        // identity (no bundle).
        assert!(BrandBundle::from_env().unwrap().is_none());
    }

    #[test]
    fn custom_branding_loads_the_named_bundle() {
        let dir = write_bundle("version: 1\nbrand:\n  primary_domain: acme.example\n");
        let path = dir.path().to_string_lossy().into_owned();
        let bundle =
            BrandBundle::from_env_with(|key| (key == CUSTOM_BRANDING_ENV).then(|| path.clone()))
                .unwrap()
                .expect("custom branding set → bundle loaded");
        assert_eq!(
            bundle.manifest.brand.primary_domain.as_deref(),
            Some("acme.example")
        );
    }

    #[test]
    fn custom_branding_fails_closed_without_a_bundle() {
        // Opted into custom branding but no bundle at the path → error, never
        // a silent fall-back to the Neon Law default.
        let error = BrandBundle::from_env_with(|key| {
            (key == CUSTOM_BRANDING_ENV).then(|| "/no/such/brand/bundle".to_string())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("navigator.yaml"));
    }

    #[test]
    fn validates_supported_fields_and_assets() {
        let dir = write_bundle("version: 1\nportal_only: true\nbrand:\n  firm: Acme Law\n  support_email: support@acme.example\n  firm_address: 1 Main St\n  base_url: https://app.acme.example\n  primary_domain: acme.example\n  consultation_url: https://acme.example/book\n  terms_url: https://acme.example/terms\n  privacy_url: https://acme.example/privacy\nassets:\n  firm_logo: firm.svg\n  firm_logo_raster: firm.png\n  static_files:\n    letterhead.css: letterhead.css\n");
        for file in ["firm.svg", "firm.png", "letterhead.css"] {
            fs::write(dir.path().join(file), "brand").unwrap();
        }
        let bundle = BrandBundle::load(dir.path()).unwrap();
        assert_eq!(bundle.manifest.brand.firm.as_deref(), Some("Acme Law"));
        assert_eq!(
            bundle.manifest.assets.firm_logo.as_deref(),
            Some(Path::new("firm.svg"))
        );
        assert_eq!(
            bundle.manifest.assets.static_files["letterhead.css"],
            PathBuf::from("letterhead.css")
        );
    }

    #[test]
    fn reports_every_invalid_field_together() {
        let dir = write_bundle("version: 99\nportal_only: true\nbrand:\n  firm: ''\n  support_email: not-an-email\n  base_url: relative\n  primary_domain: https://acme.example/path\nassets:\n  firm_logo: ../escape.svg\n  firm_logo_raster: absent.png\n  static_files:\n    ../escape.css: absent.css\n    '{wildcard}.css': absent.css\n");
        let error = BrandBundle::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("version must be 1"));
        assert!(error.contains("terms_url"));
        assert!(error.contains("brand.firm"));
        assert!(error.contains("valid email"));
        assert!(error.contains("absolute http(s) URL"));
        assert!(error.contains("bare DNS name"));
        assert!(error.contains("safe relative"));
        assert!(error.contains("safe relative public path"));
        assert!(error.contains("{wildcard}.css"));
        assert!(error.contains("absent.png"));
    }

    #[test]
    fn a_malformed_support_domain_is_rejected() {
        // `support_domain` becomes the right-hand side of `support@{HOST}`, so
        // a value carrying an `@`, a space, or no dot would ship a malformed
        // address to every client rather than fail here.
        for bad in [
            "support@acme.example",
            "acme example",
            "localhost",
            ".acme.",
        ] {
            let dir = write_bundle(&format!("version: 1\nbrand:\n  support_domain: '{bad}'\n"));
            let error = BrandBundle::load(dir.path()).unwrap_err().to_string();
            assert!(
                error.contains("brand.support_domain must be a bare hostname"),
                "{bad} must be rejected: {error}"
            );
        }
    }

    #[test]
    fn a_support_domain_alone_is_enough_to_move_the_mailbox() {
        let dir = write_bundle("version: 1\nbrand:\n  support_domain: acme.example\n");
        let bundle = BrandBundle::load(dir.path()).expect("a bare host is a complete override");
        assert_eq!(
            bundle.manifest.brand.support_domain.as_deref(),
            Some("acme.example")
        );
    }

    #[test]
    fn whitespace_only_firm_legal_entity_is_rejected() {
        // A blank legal entity would erase the name from the footer's
        // copyright line, so a whitespace-only override must fail validation
        // rather than reach the public footer.
        let dir = write_bundle("version: 1\nbrand:\n  firm_legal_entity: '   '\n");
        let error = BrandBundle::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("brand.firm_legal_entity must be omitted or non-empty"));
    }

    /// The firm-level "Admitted in …" line is retired, and `Brand` denies
    /// unknown fields, so a manifest still carrying its key fails loudly at
    /// load rather than setting a value nothing renders.
    #[test]
    fn a_manifest_still_setting_firm_bar_admissions_is_rejected() {
        let dir = write_bundle("version: 1\nbrand:\n  firm_bar_admissions:\n    - jurisdiction: Oregon\n      license_url: https://www.osbar.org/members/000\n");
        let error = BrandBundle::load(dir.path()).unwrap_err().to_string();
        assert!(
            error.contains("firm_bar_admissions"),
            "the retired key must be named in the error: {error}"
        );
    }

    #[test]
    fn invalid_offices_and_bar_licences_are_rejected() {
        // A published bar number is a verifiable claim about who may practise
        // law. A blank number, a nameless attorney, an attorney listed with no
        // licence at all, or a number whose record cannot be opened must fail
        // validation rather than reach the public footer.
        let dir = write_bundle(
            "version: 1\nbrand:\n  firm_offices:\n    - state: '  '\n      address: ''\n  firm_attorneys:\n    - name: '   '\n      licenses:\n        - jurisdiction: ''\n          number: '  '\n          license_url: not-a-url\n    - name: No Licences\n      licenses: []\n",
        );
        let error = BrandBundle::load(dir.path()).unwrap_err().to_string();
        for expected in [
            "brand.firm_offices[0].state must be non-empty",
            "brand.firm_offices[0].address must be non-empty",
            "brand.firm_attorneys[0].name must be non-empty",
            "brand.firm_attorneys[0].licenses[0].jurisdiction must be non-empty",
            "brand.firm_attorneys[0].licenses[0].number must be non-empty",
            "brand.firm_attorneys[0].licenses[0].license_url must be an absolute http(s) URL",
            "brand.firm_attorneys[1].licenses must list at least one bar licence",
        ] {
            assert!(error.contains(expected), "missing {expected}: {error}");
        }
    }

    #[test]
    fn valid_offices_and_bar_licences_pass_validation() {
        let dir = write_bundle(
            "version: 1\nbrand:\n  firm_offices:\n    - state: Idaho\n      address: 1 Main St, Boise, ID 83702\n  firm_attorneys:\n    - name: Ada Lovelace\n      licenses:\n        - jurisdiction: Idaho\n          number: '4242'\n          license_url: https://isb.idaho.gov/4242\n",
        );
        let bundle = BrandBundle::load(dir.path()).unwrap();
        assert_eq!(bundle.manifest.brand.firm_offices[0].state, "Idaho");
        assert_eq!(
            bundle.manifest.brand.firm_attorneys[0].licenses[0].number,
            "4242"
        );
    }

    #[test]
    fn malformed_manifest_is_diagnostic() {
        let dir = write_bundle("version: [");
        assert!(BrandBundle::load(dir.path())
            .unwrap_err()
            .to_string()
            .contains("parsing"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_asset_symlink_that_escapes_bundle() {
        use std::os::unix::fs::symlink;

        let outside = tempdir().unwrap();
        fs::write(outside.path().join("logo.svg"), "outside").unwrap();
        let dir = write_bundle("version: 1\nassets:\n  firm_logo: logo.svg\n");
        symlink(outside.path().join("logo.svg"), dir.path().join("logo.svg")).unwrap();

        let error = BrandBundle::load(dir.path()).unwrap_err().to_string();
        assert!(error.contains("resolves outside the bundle root"));
    }
}
