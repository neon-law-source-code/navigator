//! Data-driven brand rows (ENG-496).
//!
//! A [`Brand`] is a name a practice presents under, distinct from the
//! compiled [`views::brand::BrandKey`] registry `store` cannot depend on:
//! that enum still owns every served host, the marketing-page catalog, and
//! the compiled `Branding` copy — nothing here replaces it, and no runtime
//! brand row publishes a host or a marketing page in this cut. This table
//! is the authorization and identity record CRUD acts on: who may create a
//! brand, what it is named, and which Firm (if any) it belongs to.
//!
//! `firm_id: None` means system-wide — visible to every Firm, created only
//! by Owner. A live `firm_id` means Firm-scoped — created only by that
//! Firm's Admin DRI (`person_firm_role.is_dri`, ENG-499), who cannot also
//! create a system-wide brand through this same command. `is_law_firm` and
//! `legal_entity` are Owner's own call for a system-wide brand; a
//! Firm-scoped brand does not take them as input at all — it always
//! inherits `is_law_firm = true` and its Firm's own Entity name, because a
//! Firm-scoped brand presents that Firm's own practice.

use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::Role;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

const TABLE: &str = "brand";
const FIRM_TABLE: &str = "firm";
const PROJECT_TABLE: &str = "project";
const FIRM_BRAND_TABLE: &str = "firm_brand";
const SELECT: &str = "id, name, brand_key, firm_id, primary_color, accent_color, typeface, \
                       is_law_firm, legal_entity, logo_object_key, logo_content_type, \
                       font_family, font_object_key, font_licence, inserted_at, updated_at";

/// The closed list of open-source licences a brand's uploaded font may be
/// attested under (ENG-586). Matches the schema `ASSERT` on
/// `brand.font_licence`; this is the friendlier Rust-side refusal before a
/// bad value ever reaches the database.
pub const FONT_LICENCES: &[&str] = &["OFL-1.1", "Apache-2.0", "UFL-1.0"];

/// WCAG AA contrast floor for a brand's primary colour against its
/// best-contrasting on-primary (white or black).
const MIN_CONTRAST: f64 = 4.5;

/// Parse `#rrggbb` into sRGB bytes. A self-contained copy of
/// `views::brand_presentation::parse_hex` — `store` does not depend on
/// `views`, so the small, pure colour-math primitives this validation needs
/// are duplicated rather than imported (the same reasoning
/// `store::firms::CLOSED_BRAND_KEYS` gives for its own copy of `BrandKey::ALL`).
fn parse_hex(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |offset: usize| u8::from_str_radix(&hex[offset..offset + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn channel_luminance(value: u8) -> f64 {
    let c = f64::from(value) / 255.0;
    if c <= 0.039_28 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance([r, g, b]: [u8; 3]) -> f64 {
    0.2126 * channel_luminance(r) + 0.7152 * channel_luminance(g) + 0.0722 * channel_luminance(b)
}

fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

/// Validate a brand's proposed primary hex: well-formed `#rrggbb`, and its
/// best on-primary contrast (white or black, whichever is higher) clears
/// WCAG AA 4.5:1 (ENG-586). Refuses before any write, naming the ratio so the
/// caller can state it.
///
/// In practice [`BrandError::InsufficientContrast`] is unreachable for a
/// well-formed hex: `max(contrast(hex, white), contrast(hex, black))` has a
/// mathematical floor of `sqrt(1.05 * 0.05) / 0.05 ≈ 4.58`, reached only at
/// the exact luminance where the two are equal — every other value clears it
/// by more. `create_accepts_every_well_formed_hex_because_the_contrast_floor_always_clears`
/// proves this against several deliberately "unreadable-looking" hexes. The
/// check stays in place because it costs nothing, matches what ENG-586
/// specified, and is the correct shape if the threshold or formula ever
/// changes — but it is not, today, a gate that can refuse an ill-considered
/// brand colour. A gate that can actually reject a pale or low-saturation
/// primary would need to check contrast against a fixed background (the
/// page's own light-mode surface) rather than against the best of two
/// self-selected extremes.
fn validate_primary_hex(value: &str) -> Result<(), BrandError> {
    let rgb = parse_hex(value).ok_or_else(|| BrandError::InvalidHex(value.to_string()))?;
    let white = contrast_ratio(rgb, [0xff, 0xff, 0xff]);
    let black = contrast_ratio(rgb, [0x00, 0x00, 0x00]);
    let best = white.max(black);
    if best < MIN_CONTRAST {
        return Err(BrandError::InsufficientContrast {
            hex: value.to_string(),
            ratio: best,
        });
    }
    Ok(())
}

/// A data-driven brand row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brand {
    pub id: Uuid,
    pub name: String,
    pub key: String,
    /// `None` is system-wide; `Some(firm_id)` is scoped to that Firm.
    pub firm_id: Option<Uuid>,
    pub primary_color: Option<String>,
    pub accent_color: Option<String>,
    pub typeface: Option<String>,
    pub is_law_firm: bool,
    pub legal_entity: Option<String>,
    /// The uploaded logo's public-bucket object key, when one exists.
    pub logo_object_key: Option<String>,
    /// The uploaded logo's validated content type (`image/png` or
    /// `image/svg+xml`).
    pub logo_content_type: Option<String>,
    /// The uploaded font's CSS `font-family` name, when `typeface` is
    /// `"uploaded"`.
    pub font_family: Option<String>,
    /// The uploaded `.woff2`'s public-bucket object key.
    pub font_object_key: Option<String>,
    /// The attested open licence the uploaded font is under. See
    /// [`FONT_LICENCES`].
    pub font_licence: Option<String>,
    pub inserted_at: String,
    pub updated_at: String,
}

#[derive(SurrealValue)]
struct BrandRow {
    id: surrealdb::types::RecordId,
    name: String,
    brand_key: String,
    firm_id: Option<surrealdb::types::RecordId>,
    primary_color: Option<String>,
    accent_color: Option<String>,
    typeface: Option<String>,
    is_law_firm: bool,
    legal_entity: Option<String>,
    logo_object_key: Option<String>,
    logo_content_type: Option<String>,
    font_family: Option<String>,
    font_object_key: Option<String>,
    font_licence: Option<String>,
    inserted_at: String,
    updated_at: String,
}

impl BrandRow {
    fn into_brand(self) -> Option<Brand> {
        Some(Brand {
            id: record_uuid(&self.id)?,
            name: self.name,
            key: self.brand_key,
            firm_id: match self.firm_id.as_ref() {
                Some(id) => Some(record_uuid(id)?),
                None => None,
            },
            primary_color: self.primary_color,
            accent_color: self.accent_color,
            typeface: self.typeface,
            is_law_firm: self.is_law_firm,
            legal_entity: self.legal_entity,
            logo_object_key: self.logo_object_key,
            logo_content_type: self.logo_content_type,
            font_family: self.font_family,
            font_object_key: self.font_object_key,
            font_licence: self.font_licence,
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

/// Inputs for creating a [`Brand`].
///
/// `is_law_firm` and `legal_entity` are honored only for a system-wide
/// request (`firm_id: None`) — a Firm-scoped request always computes both
/// from the target Firm and ignores whatever these fields carry.
#[derive(Debug, Clone, Default)]
pub struct NewBrand {
    pub name: String,
    pub key: String,
    pub firm_id: Option<Uuid>,
    pub primary_color: Option<String>,
    pub accent_color: Option<String>,
    pub typeface: Option<String>,
    pub is_law_firm: bool,
    pub legal_entity: Option<String>,
}

/// A partial edit to a [`Brand`]'s presentation fields. Never touches
/// `firm_id`, `is_law_firm`, or `legal_entity` — those are fixed at
/// creation and inherited, not edited. Never touches the logo or font
/// object-storage fields either — [`set_logo`] and [`set_font`] are the only
/// writers for those, since only a native multipart upload can put bytes in
/// the object matching them.
#[derive(Debug, Clone, Default)]
pub struct BrandEdit {
    pub name: Option<String>,
    /// A validated `#rrggbb` hex (ENG-586), gated behind
    /// [`validate_primary_hex`]. `Some(None)` clears it (falls back to the
    /// compiled default); `Some(Some(hex))` sets it.
    pub primary_color: Option<Option<String>>,
    pub accent_color: Option<Option<String>>,
    pub typeface: Option<Option<String>>,
    /// The CSS `font-family` name for an already-uploaded font. Renaming it
    /// does not touch the uploaded object — see [`set_font`].
    pub font_family: Option<Option<String>>,
}

/// Errors from the brand command seam.
#[derive(Debug, thiserror::Error)]
pub enum BrandError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Firm(#[from] crate::firms::FirmError),
    #[error("writing a brand returned no usable row")]
    WriteReturnedNothing,
    #[error("no brand {0}")]
    NoSuchBrand(Uuid),
    #[error("no firm {0}")]
    NoSuchFirm(Uuid),
    #[error("that brand name is already taken")]
    DuplicateName,
    #[error("that brand key is already taken")]
    DuplicateKey,
    /// Owner alone creates a system-wide brand; a Firm's Admin DRI alone
    /// creates one scoped to their own Firm. Every other actor, and an
    /// Admin DRI naming a different Firm, is refused this.
    #[error("you may not create, edit, or delete this brand")]
    NotAuthorized,
    /// The proposed `primary_color` is not a well-formed `#rrggbb` hex.
    #[error("{0} is not a valid #rrggbb hex colour")]
    InvalidHex(String),
    /// The proposed `primary_color`'s best on-primary contrast (white or
    /// black) falls short of WCAG AA 4.5:1 (ENG-586).
    #[error("{hex}'s best on-primary contrast is {ratio:.1}:1; it must be at least 4.5:1")]
    InsufficientContrast { hex: String, ratio: f64 },
    /// The proposed font licence is not one of [`FONT_LICENCES`].
    #[error("font licence must be one of {}", FONT_LICENCES.join(", "))]
    InvalidFontLicence(String),
    /// [`delete`] is refused while a `firm_brand` or `project.brand` row
    /// still names this brand's key — deleting it would leave those rows
    /// pointing at nothing.
    #[error("that brand is still worn by a firm or named by a project")]
    StillReferenced,
}

impl BrandError {
    /// The message to show a human on the brand create/edit/delete forms.
    /// Kept here so every adapter renders the same wording per refusal, the
    /// same convention [`crate::firms::FirmError::user_message`] follows.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::InvalidHex(hex) => format!("{hex} is not a valid #rrggbb hex colour."),
            Self::InsufficientContrast { hex, ratio } => format!(
                "{hex}'s best on-primary contrast is {ratio:.1}:1; it must be at least 4.5:1."
            ),
            Self::InvalidFontLicence(_) => {
                format!("Pick a font licence: {}.", FONT_LICENCES.join(", "))
            }
            Self::StillReferenced => {
                "That brand is still worn by a firm or named by a project.".to_string()
            }
            Self::NotAuthorized => "You may not create, edit, or delete this brand.".to_string(),
            Self::NoSuchBrand(_) => "That brand could not be found.".to_string(),
            Self::NoSuchFirm(_) => "That firm could not be found.".to_string(),
            Self::DuplicateName => "That brand name is already taken.".to_string(),
            Self::DuplicateKey => "That brand key is already taken.".to_string(),
            Self::Db(_) | Self::Firm(_) | Self::WriteReturnedNothing => {
                "Could not save brand.".to_string()
            }
        }
    }
}

fn classify_write(error: surrealdb::Error) -> BrandError {
    match retry::unique_violation(&error) {
        Some("brand_name") => BrandError::DuplicateName,
        Some("brand_key_unique") => BrandError::DuplicateKey,
        _ => BrandError::Db(error),
    }
}

async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, BrandError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// Whether `actor` may create, edit, or delete a brand at `target_firm_id`
/// (`None` for system-wide).
///
/// Side-effect-free. Owner passes only the system-wide case; a Firm's own
/// Admin DRI passes only that Firm's case. Every other combination —
/// including Owner attempting a Firm-scoped brand, or that Firm's non-DRI
/// Admin — is refused.
async fn authorize(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    target_firm_id: Option<Uuid>,
) -> Result<(), BrandError> {
    match target_firm_id {
        None => {
            if actor_role == Role::Owner {
                Ok(())
            } else {
                Err(BrandError::NotAuthorized)
            }
        }
        Some(firm_id) => {
            if crate::firms::find_by_id(surreal, firm_id).await?.is_none() {
                return Err(BrandError::NoSuchFirm(firm_id));
            }
            let Some(person_id) = actor_person_id else {
                return Err(BrandError::NotAuthorized);
            };
            match crate::firms::membership_for_person(surreal, person_id, firm_id).await? {
                Some(row)
                    if row.is_dri && row.membership == crate::firms::FirmMembership::Admin =>
                {
                    Ok(())
                }
                _ => Err(BrandError::NotAuthorized),
            }
        }
    }
}

/// Create a brand. See the module doc for the authorization split and what
/// a Firm-scoped request inherits.
pub async fn create(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    input: &NewBrand,
) -> Result<Brand, BrandError> {
    authorize(surreal, actor_role, actor_person_id, input.firm_id).await?;
    if let Some(hex) = input.primary_color.as_deref() {
        validate_primary_hex(hex)?;
    }

    let (is_law_firm, legal_entity) = match input.firm_id {
        None => (input.is_law_firm, input.legal_entity.clone()),
        Some(firm_id) => {
            // `authorize` already proved this Firm exists.
            let firm = crate::firms::find_by_id(surreal, firm_id)
                .await?
                .ok_or(BrandError::NoSuchFirm(firm_id))?;
            let entity_name = match firm.entity_id {
                Some(entity_id) => crate::entities::find_by_id(surreal, entity_id)
                    .await
                    .map_err(crate::firms::FirmError::from)?
                    .map(|entity| entity.name),
                None => None,
            };
            (true, entity_name)
        }
    };

    let id = Uuid::now_v7();
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "CREATE $id SET name = $name, brand_key = $brand_key, firm_id = $firm_id, \
                 primary_color = $primary_color, accent_color = $accent_color, typeface = $typeface, \
                 is_law_firm = $is_law_firm, legal_entity = $legal_entity, \
                 inserted_at = $now, updated_at = $now RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)))
            .bind(("name", input.name.clone()))
            .bind(("brand_key", input.key.clone()))
            .bind(("firm_id", input.firm_id.map(|fid| record_id(FIRM_TABLE, fid))))
            .bind(("primary_color", input.primary_color.clone()))
            .bind(("accent_color", input.accent_color.clone()))
            .bind(("typeface", input.typeface.clone()))
            .bind(("is_law_firm", is_law_firm))
            .bind(("legal_entity", legal_entity.clone()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<BrandRow> = response.take(0)?;
    row.and_then(BrandRow::into_brand)
        .ok_or(BrandError::WriteReturnedNothing)
}

/// Find a brand by id.
pub async fn find_by_id(surreal: &SurrealDb, id: Uuid) -> Result<Option<Brand>, BrandError> {
    let mut response = surreal
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<BrandRow> = response.take(0)?;
    Ok(row.and_then(BrandRow::into_brand))
}

/// Find a brand by its unique key.
pub async fn find_by_key(surreal: &SurrealDb, key: &str) -> Result<Option<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE brand_key = $key LIMIT 1"
        ))
        .bind(("key", key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().find_map(BrandRow::into_brand))
}

/// Every system-wide brand (`firm_id IS NONE`), name then id — the set
/// every Firm sees regardless of its own scoped brands.
pub async fn system_wide(surreal: &SurrealDb) -> Result<Vec<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE firm_id IS NONE ORDER BY name, id"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(BrandRow::into_brand).collect())
}

/// Every Firm-scoped brand across the whole deployment, name then id — the
/// Owner-only inventory `/app/brands` lists alongside [`system_wide`]. Never
/// used for a Firm-scoped viewer's own listing, which stays [`for_firm`].
pub async fn all_firm_scoped(surreal: &SurrealDb) -> Result<Vec<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE firm_id IS NOT NONE ORDER BY name, id"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(BrandRow::into_brand).collect())
}

/// Every brand scoped to this Firm — never another Firm's, and never the
/// system-wide set (use [`system_wide`] for that).
pub async fn for_firm(surreal: &SurrealDb, firm_id: Uuid) -> Result<Vec<Brand>, BrandError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE firm_id = $firm_id ORDER BY name, id"
        ))
        .bind(("firm_id", record_id(FIRM_TABLE, firm_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<BrandRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(BrandRow::into_brand).collect())
}

/// Edit a brand's presentation fields. Authorized exactly as [`create`]
/// would be for this brand's existing scope: Owner for a system-wide
/// brand, that Firm's own Admin DRI for a Firm-scoped one.
pub async fn update(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    brand_id: Uuid,
    input: &BrandEdit,
) -> Result<Brand, BrandError> {
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
    authorize(surreal, actor_role, actor_person_id, existing.firm_id).await?;
    if let Some(Some(hex)) = input.primary_color.as_ref() {
        validate_primary_hex(hex)?;
    }

    let mut assignments: Vec<&str> = vec!["updated_at = $updated_at"];
    if input.name.is_some() {
        assignments.push("name = $name");
    }
    if input.primary_color.is_some() {
        assignments.push("primary_color = $primary_color");
    }
    if input.accent_color.is_some() {
        assignments.push("accent_color = $accent_color");
    }
    if input.typeface.is_some() {
        assignments.push("typeface = $typeface");
    }
    if input.font_family.is_some() {
        assignments.push("font_family = $font_family");
    }
    if assignments.len() == 1 {
        return Ok(existing);
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET {} RETURN {SELECT}",
                assignments.join(", ")
            ))
            .bind(("id", record_id(TABLE, brand_id)))
            .bind(("updated_at", now.clone()))
            .bind(("name", input.name.clone()))
            .bind(("primary_color", input.primary_color.clone()))
            .bind(("accent_color", input.accent_color.clone()))
            .bind(("typeface", input.typeface.clone()))
            .bind(("font_family", input.font_family.clone()))
    })
    .await?;
    let row: Option<BrandRow> = response.take(0)?;
    row.and_then(BrandRow::into_brand)
        .ok_or(BrandError::WriteReturnedNothing)
}

/// Attach an uploaded logo. Authorized exactly as [`update`]. `object_key` is
/// the public assets-bucket key the caller already wrote the validated bytes
/// to; `content_type` is `image/png` or `image/svg+xml`. Overwrites any prior
/// logo — the caller is responsible for deleting the superseded object if it
/// wants the bucket tidy.
pub async fn set_logo(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    brand_id: Uuid,
    object_key: &str,
    content_type: &str,
) -> Result<Brand, BrandError> {
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
    authorize(surreal, actor_role, actor_person_id, existing.firm_id).await?;

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET logo_object_key = $object_key, \
                 logo_content_type = $content_type, updated_at = $now RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, brand_id)))
            .bind(("object_key", object_key.to_string()))
            .bind(("content_type", content_type.to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<BrandRow> = response.take(0)?;
    row.and_then(BrandRow::into_brand)
        .ok_or(BrandError::WriteReturnedNothing)
}

/// Attach an uploaded font. Authorized exactly as [`update`]. `licence` must
/// be one of [`FONT_LICENCES`] — refused before any write otherwise, the
/// friendlier twin of the schema's own `ASSERT`. Setting `typeface` to
/// `"uploaded"` is the caller's job (the presentational form field), not this
/// function's — a brand may upload a font and keep wearing a catalog
/// typeface until it switches over.
pub async fn set_font(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    brand_id: Uuid,
    family: &str,
    object_key: &str,
    licence: &str,
) -> Result<Brand, BrandError> {
    if !FONT_LICENCES.contains(&licence) {
        return Err(BrandError::InvalidFontLicence(licence.to_string()));
    }
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
    authorize(surreal, actor_role, actor_person_id, existing.firm_id).await?;

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET font_family = $family, font_object_key = $object_key, \
                 font_licence = $licence, updated_at = $now RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, brand_id)))
            .bind(("family", family.to_string()))
            .bind(("object_key", object_key.to_string()))
            .bind(("licence", licence.to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<BrandRow> = response.take(0)?;
    row.and_then(BrandRow::into_brand)
        .ok_or(BrandError::WriteReturnedNothing)
}

/// Whether any `firm_brand` or `project.brand` row still names `brand_key`.
async fn is_referenced(surreal: &SurrealDb, brand_key: &str) -> Result<bool, BrandError> {
    #[derive(SurrealValue)]
    struct IdRow {
        id: surrealdb::types::RecordId,
    }
    let mut firm_brands = surreal
        .query(format!(
            "SELECT id FROM {FIRM_BRAND_TABLE} WHERE brand_key = $key LIMIT 1"
        ))
        .bind(("key", brand_key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let firm_brand_rows: Vec<IdRow> = firm_brands.take(0)?;
    if !firm_brand_rows.is_empty() {
        return Ok(true);
    }
    let mut projects = surreal
        .query(format!(
            "SELECT id FROM {PROJECT_TABLE} WHERE brand = $key LIMIT 1"
        ))
        .bind(("key", brand_key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let project_rows: Vec<IdRow> = projects.take(0)?;
    Ok(!project_rows.is_empty())
}

/// Delete a brand. Authorized exactly as [`update`]. Refused with
/// [`BrandError::StillReferenced`] while any `firm_brand` or `project.brand`
/// row still names this brand's key — deleting it would orphan those rows.
/// Deleting an unworn, unnamed brand removes only the row; its uploaded
/// logo/font objects in the public assets bucket are the caller's to clean up
/// (this function has no bucket handle).
pub async fn delete(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    brand_id: Uuid,
) -> Result<(), BrandError> {
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
    authorize(surreal, actor_role, actor_person_id, existing.firm_id).await?;
    if is_referenced(surreal, &existing.key).await? {
        return Err(BrandError::StillReferenced);
    }
    writing(|| {
        surreal
            .query("DELETE $id")
            .bind(("id", record_id(TABLE, brand_id)))
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firms::{FirmMembership, NewFirm, NewPersonFirmRole};
    use crate::persons::NewPerson;
    use crate::test_support::{mem_surreal, seed_entity};

    async fn admin_dri_person(db: &SurrealDb) -> Uuid {
        crate::persons::create(
            db,
            &NewPerson::with_role(
                "Admin DRI",
                format!("brand-admin-dri-{}@example.com", Uuid::now_v7()),
                Role::Admin,
            ),
        )
        .await
        .unwrap()
        .id
    }

    async fn practice(db: &SurrealDb, name: &str) -> (crate::firms::Firm, Uuid) {
        let entity_id = seed_entity(db).await;
        let admin = admin_dri_person(db).await;
        let firm = crate::firms::create(
            db,
            &NewFirm {
                name: name.to_string(),
                status: "active".to_string(),
                entity_id,
                admin_dri_person_id: admin,
            },
        )
        .await
        .unwrap();
        (firm, admin)
    }

    #[tokio::test]
    async fn owner_creates_a_system_wide_brand_visible_to_every_firm() {
        let db = mem_surreal().await;
        let brand = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Acme Law".to_string(),
                key: "acme-law".to_string(),
                is_law_firm: true,
                legal_entity: Some("Shook Law PLLC".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(brand.firm_id, None);
        assert!(brand.is_law_firm);
        assert_eq!(brand.legal_entity.as_deref(), Some("Shook Law PLLC"));

        let (firm, _admin) = practice(&db, "Any Practice").await;
        assert!(system_wide(&db)
            .await
            .unwrap()
            .iter()
            .any(|b| b.id == brand.id));
        assert!(for_firm(&db, firm.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_firm_s_admin_dri_creates_a_scoped_brand_inheriting_the_firm_s_entity() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Scoped Practice").await;
        // The Firm's own name (`practice`'s argument) and its Entity's name
        // are deliberately distinct here — `seed_entity` names the Entity
        // itself — so this proves inheritance reads the real linked Entity
        // rather than coincidentally matching the Firm's own name.
        let entity_name = crate::entities::find_by_id(&db, firm.entity_id.unwrap())
            .await
            .unwrap()
            .unwrap()
            .name;

        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Scoped Brand".to_string(),
                key: "scoped-brand".to_string(),
                firm_id: Some(firm.id),
                // Submitted but must be ignored/overridden for a Firm-scoped brand.
                is_law_firm: false,
                legal_entity: Some("Someone Else PLLC".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(brand.firm_id, Some(firm.id));
        assert!(
            brand.is_law_firm,
            "a Firm-scoped brand is always a law firm"
        );
        assert_eq!(brand.legal_entity.as_deref(), Some(entity_name.as_str()));
        assert_eq!(for_firm(&db, firm.id).await.unwrap(), vec![brand]);
    }

    /// Owner cannot create a Firm-scoped brand; a Firm's Admin DRI cannot
    /// create a system-wide one; a non-DRI Admin of the Firm, and an Admin
    /// DRI of a *different* Firm, may create neither.
    #[tokio::test]
    async fn create_refuses_every_mismatched_actor() {
        let db = mem_surreal().await;
        let (firm_a, admin_a) = practice(&db, "Practice A").await;
        let (_firm_b, admin_b) = practice(&db, "Practice B").await;

        let err = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Owner Scoped Attempt".to_string(),
                key: "owner-scoped-attempt".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let err = create(
            &db,
            Role::Admin,
            Some(admin_a),
            &NewBrand {
                name: "Admin System Wide Attempt".to_string(),
                key: "admin-system-wide-attempt".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let err = create(
            &db,
            Role::Admin,
            Some(admin_b),
            &NewBrand {
                name: "Cross Firm Attempt".to_string(),
                key: "cross-firm-attempt".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let non_dri_admin = crate::persons::create(
            &db,
            &NewPerson::with_role("Non DRI Admin", "non-dri-admin@example.com", Role::Admin),
        )
        .await
        .unwrap();
        crate::firms::add_membership(
            &db,
            &NewPersonFirmRole {
                person_id: non_dri_admin.id,
                firm_id: firm_a.id,
                membership: FirmMembership::Admin,
                is_dri: false,
            },
        )
        .await
        .unwrap();
        let err = create(
            &db,
            Role::Admin,
            Some(non_dri_admin.id),
            &NewBrand {
                name: "Non DRI Attempt".to_string(),
                key: "non-dri-attempt".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        for role in [Role::Lawyer, Role::Clerk, Role::Client] {
            let err = create(
                &db,
                role,
                None,
                &NewBrand {
                    name: format!("{role:?} Attempt"),
                    key: format!("{role:?}-attempt").to_lowercase(),
                    ..NewBrand::default()
                },
            )
            .await
            .unwrap_err();
            assert!(matches!(err, BrandError::NotAuthorized), "{role:?}");
        }
    }

    #[tokio::test]
    async fn name_and_key_are_each_globally_unique() {
        let db = mem_surreal().await;
        create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "First".to_string(),
                key: "first".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let duplicate_name = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "First".to_string(),
                key: "second".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate_name, BrandError::DuplicateName));

        let duplicate_key = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Second".to_string(),
                key: "first".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate_key, BrandError::DuplicateKey));
    }

    #[tokio::test]
    async fn update_and_delete_are_authorized_like_create_and_leave_scope_fixed() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Editable Practice").await;
        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Editable Brand".to_string(),
                key: "editable-brand".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let err = update(
            &db,
            Role::Owner,
            None,
            brand.id,
            &BrandEdit {
                name: Some("Hijacked".to_string()),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));

        let edited = update(
            &db,
            Role::Admin,
            Some(admin),
            brand.id,
            &BrandEdit {
                primary_color: Some(Some("#123456".to_string())),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(edited.name, "Editable Brand");
        assert_eq!(edited.primary_color.as_deref(), Some("#123456"));
        assert_eq!(edited.firm_id, Some(firm.id));

        let err = delete(&db, Role::Owner, None, brand.id).await.unwrap_err();
        assert!(matches!(err, BrandError::NotAuthorized));
        delete(&db, Role::Admin, Some(admin), brand.id)
            .await
            .unwrap();
        assert!(find_by_id(&db, brand.id).await.unwrap().is_none());
    }

    /// ENG-586: a malformed hex is refused before any write, and a
    /// well-formed one that clears the gate is stored on both create and
    /// update. `#007c91` (the compiled `neon` brand's own primary) is the
    /// known-good reference: `views::brand_presentation`'s own test proves it
    /// clears WCAG AA against white, so this is not an arbitrary fixture.
    #[tokio::test]
    async fn create_and_update_validate_the_primary_hex() {
        let db = mem_surreal().await;

        let err = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Malformed Hex".to_string(),
                key: "malformed-hex".to_string(),
                primary_color: Some("not-a-hex".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::InvalidHex(hex) if hex == "not-a-hex"));

        let brand = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Valid Hex".to_string(),
                key: "valid-hex".to_string(),
                primary_color: Some("#007c91".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(brand.primary_color.as_deref(), Some("#007c91"));

        let err = update(
            &db,
            Role::Owner,
            None,
            brand.id,
            &BrandEdit {
                primary_color: Some(Some("#zzzzzz".to_string())),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::InvalidHex(_)));
    }

    /// ENG-586: the "best of white or black" contrast the gate checks has a
    /// mathematical floor of `sqrt(1.05 * 0.05) / 0.05 ≈ 4.58` — every
    /// well-formed hex, however pale or saturated, clears WCAG AA 4.5:1
    /// against *some* choice of on-primary, so [`BrandError::InsufficientContrast`]
    /// is unreachable for a well-formed value. A pale yellow that reads as
    /// "low contrast" against white still clears the gate against black.
    /// This is the reachability proof for that floor, so a future change to
    /// the formula or the threshold surfaces here rather than silently
    /// making the gate meaningless (or, if the threshold is ever lowered
    /// below the floor, silently making it unreachable in the other
    /// direction).
    #[tokio::test]
    async fn create_accepts_every_well_formed_hex_because_the_contrast_floor_always_clears() {
        let db = mem_surreal().await;
        for (key, hex) in [
            ("pale-yellow", "#f5f5a0"),
            ("near-white", "#fefefe"),
            ("near-black", "#010101"),
            ("mid-gray", "#808080"),
        ] {
            let brand = create(
                &db,
                Role::Owner,
                None,
                &NewBrand {
                    name: key.to_string(),
                    key: key.to_string(),
                    primary_color: Some(hex.to_string()),
                    ..NewBrand::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("{hex} must clear the contrast floor: {error}"));
            assert_eq!(brand.primary_color.as_deref(), Some(hex));
        }
    }

    /// ENG-586: `set_font` refuses a licence outside the closed list before
    /// any write, and stores a valid one alongside the family and object key.
    #[tokio::test]
    async fn set_font_validates_the_licence_and_stores_the_upload() {
        let db = mem_surreal().await;
        let brand = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Fontable".to_string(),
                key: "fontable".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let err = set_font(
            &db,
            Role::Owner,
            None,
            brand.id,
            "Custom Sans",
            "fonts/brands/fontable/abc123.woff2",
            "MIT",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::InvalidFontLicence(l) if l == "MIT"));

        let updated = set_font(
            &db,
            Role::Owner,
            None,
            brand.id,
            "Custom Sans",
            "fonts/brands/fontable/abc123.woff2",
            "OFL-1.1",
        )
        .await
        .unwrap();
        assert_eq!(updated.font_family.as_deref(), Some("Custom Sans"));
        assert_eq!(
            updated.font_object_key.as_deref(),
            Some("fonts/brands/fontable/abc123.woff2")
        );
        assert_eq!(updated.font_licence.as_deref(), Some("OFL-1.1"));
    }

    /// ENG-586: `set_logo` is authorized exactly like `update`, and stores the
    /// object key and content type.
    #[tokio::test]
    async fn set_logo_stores_the_object_key_and_content_type() {
        let db = mem_surreal().await;
        let brand = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Logoable".to_string(),
                key: "logoable".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let updated = set_logo(
            &db,
            Role::Owner,
            None,
            brand.id,
            "brands/logoable/logo.png",
            "image/png",
        )
        .await
        .unwrap();
        assert_eq!(
            updated.logo_object_key.as_deref(),
            Some("brands/logoable/logo.png")
        );
        assert_eq!(updated.logo_content_type.as_deref(), Some("image/png"));
    }

    /// ENG-586: the Owner-only inventory reads every Firm-scoped brand across
    /// every Firm, distinct from `for_firm`'s single-Firm scope.
    #[tokio::test]
    async fn all_firm_scoped_spans_every_firm() {
        let db = mem_surreal().await;
        let (firm_a, admin_a) = practice(&db, "Practice A").await;
        let (firm_b, admin_b) = practice(&db, "Practice B").await;
        let a = create(
            &db,
            Role::Admin,
            Some(admin_a),
            &NewBrand {
                name: "Brand A".to_string(),
                key: "brand-a".to_string(),
                firm_id: Some(firm_a.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        let b = create(
            &db,
            Role::Admin,
            Some(admin_b),
            &NewBrand {
                name: "Brand B".to_string(),
                key: "brand-b".to_string(),
                firm_id: Some(firm_b.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        let system_wide_brand = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "System Wide".to_string(),
                key: "system-wide".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let all = all_firm_scoped(&db).await.unwrap();
        assert_eq!(all, vec![a, b]);
        assert!(!all.contains(&system_wide_brand));
    }

    /// ENG-586: deleting a brand a Firm wears, or that a Project names, is
    /// refused; deleting an unworn, unnamed brand removes the row. Uses
    /// runtime-created keys rather than the three compiled ones: ENG-587
    /// drops `firm_brand`/`project.brand`'s closed `ASSERT`, so the
    /// reference check must hold for any key, not only the legacy three
    /// (which a fresh test engine now seeds as `brand` rows anyway — see
    /// `store::surreal::test_support::mem`).
    #[tokio::test]
    async fn delete_is_refused_while_referenced_by_a_firm_or_a_project() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Referencing Practice").await;

        let worn = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Worn Brand".to_string(),
                key: "worn-brand".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        crate::firms::attach_brand(&db, firm.id, "worn-brand")
            .await
            .unwrap();
        let err = delete(&db, Role::Owner, None, worn.id).await.unwrap_err();
        assert!(matches!(err, BrandError::StillReferenced));

        let named = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Named Brand".to_string(),
                key: "named-brand".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        let entity_id = crate::test_support::seed_entity(&db).await;
        crate::projects::create(
            &db,
            &crate::projects::NewProject {
                code: "brand-reference-project".to_string(),
                name: "Brand Reference Project".to_string(),
                status: "open".to_string(),
                brand: "named-brand".to_string(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let err = delete(&db, Role::Owner, None, named.id).await.unwrap_err();
        assert!(matches!(err, BrandError::StillReferenced));

        // An admin-created, unworn, unnamed brand deletes cleanly.
        let unworn = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Unworn Brand".to_string(),
                key: "unworn-brand".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        delete(&db, Role::Admin, Some(admin), unworn.id)
            .await
            .unwrap();
        assert!(find_by_id(&db, unworn.id).await.unwrap().is_none());
    }
}
