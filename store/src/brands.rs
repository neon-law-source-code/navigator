//! Data-driven brand rows (ENG-496, ENG-659).
//!
//! A [`Brand`] is a name a practice presents under, distinct from the
//! compiled [`views::brand::BrandKey`] registry `store` cannot depend on:
//! that enum still owns every served host, the marketing-page catalog, and
//! the compiled `Branding` copy — nothing here replaces it, and no runtime
//! brand row publishes a host or a marketing page in this cut. This table
//! is the authorization and identity record CRUD acts on: who may create a
//! brand, what it is named, and which Firm it belongs to.
//!
//! Every brand row is Firm-scoped (ENG-659): `firm_id` is required, and
//! [`create`] refuses `firm_id: None` before anything else, for every actor
//! including Owner. Only a Firm's own Admin DRI (`person_firm_role.is_dri`,
//! ENG-499) may create that Firm's brand. `is_law_firm` and `legal_entity`
//! are not caller input at all — a Firm-scoped brand always inherits
//! `is_law_firm = true` and its Firm's own Entity name, because a
//! Firm-scoped brand presents that Firm's own practice. Owner still governs
//! *existing* brands on every Firm ([`authorize_existing`]/[`update`]) — the
//! restriction is on creating a new row, not on editing one.
//!
//! A `firm_id IS NONE` row is a historical fact only: every compiled house
//! brand used to migrate into the table this way, before ENG-659's schema
//! migration backfilled each one onto the Firm that wears it
//! (`store::schema::backfill_brand_firm_id`) and tightened the column to a
//! required `record<firm>`. [`system_wide`] and [`all_firm_scoped`] stay
//! read-only historical-compatibility helpers; nothing can write a new
//! `firm_id: None` row through this module.

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

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const BLACK: [u8; 3] = [0x00, 0x00, 0x00];

/// The light-mode page surface every brand's primary colour actually
/// renders against — `--nav-color-bg: #ffffff` in
/// `server/public/css/tokens.css`, the same default
/// `views::brand_presentation::scheme_bg` falls back to for a scheme with no
/// explicit `bg`. Duplicated as a literal for the reason [`parse_hex`]'s doc
/// comment gives: `store` does not depend on `views` or the CSS.
const LIGHT_PAGE_SURFACE: [u8; 3] = WHITE;

/// WCAG AA contrast floor for a brand's primary colour against its
/// deterministically-chosen on-primary text colour (ENG-629).
const MIN_ON_PRIMARY_CONTRAST: f64 = 4.5;

/// WCAG UI-component contrast floor (1.4.11) for a brand's primary colour
/// against the light page surface it renders on (ENG-629).
const MIN_SURFACE_CONTRAST: f64 = 3.0;

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

/// Validate a brand's proposed primary hex against the two fixed
/// backgrounds it actually renders on (ENG-629): well-formed `#rrggbb`
/// first, then its deterministically-chosen on-primary text colour — white
/// or black, whichever contrasts more, never "the best of both" as a single
/// opaque ratio — must clear WCAG AA 4.5:1, and the primary itself must
/// clear 3:1 against [`LIGHT_PAGE_SURFACE`]. Refuses before any write,
/// naming which background failed and the ratio so the caller can state it.
///
/// The on-primary check alone cannot reject a well-formed hex:
/// `max(contrast(hex, white), contrast(hex, black))` has a mathematical
/// floor of `sqrt(1.05 * 0.05) / 0.05 ≈ 4.58`, reached only at the exact
/// luminance where the two are equal — every other value clears 4.5:1 by
/// more. The page-surface check is what can actually refuse an
/// ill-considered brand colour: a pale or low-saturation primary can read
/// fine against whichever on-primary text it picks while all but
/// disappearing against the white page around it.
/// `create_and_update_refuse_a_pale_primary_that_would_clear_the_old_best_of_gate`
/// proves this failure mode; the former (ENG-586) test proved only that the
/// old best-of check could never reject anything.
fn validate_primary_hex(value: &str) -> Result<(), BrandError> {
    let rgb = parse_hex(value).ok_or_else(|| BrandError::InvalidHex(value.to_string()))?;

    let (on_primary, against) = if contrast_ratio(rgb, WHITE) >= contrast_ratio(rgb, BLACK) {
        (WHITE, "white on-primary text")
    } else {
        (BLACK, "black on-primary text")
    };
    let text_ratio = contrast_ratio(rgb, on_primary);
    if text_ratio < MIN_ON_PRIMARY_CONTRAST {
        return Err(BrandError::InsufficientContrast {
            hex: value.to_string(),
            against,
            ratio: text_ratio,
            required: MIN_ON_PRIMARY_CONTRAST,
        });
    }

    let surface_ratio = contrast_ratio(rgb, LIGHT_PAGE_SURFACE);
    if surface_ratio < MIN_SURFACE_CONTRAST {
        return Err(BrandError::InsufficientContrast {
            hex: value.to_string(),
            against: "the light page surface",
            ratio: surface_ratio,
            required: MIN_SURFACE_CONTRAST,
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

/// Inputs for creating a [`Brand`]. `firm_id` is required — [`create`]
/// refuses `None` before anything else (ENG-659). `is_law_firm` and
/// `legal_entity` are not caller input at all: every brand is always
/// computed as `is_law_firm = true` with its target Firm's own Entity name.
#[derive(Debug, Clone, Default)]
pub struct NewBrand {
    pub name: String,
    pub key: String,
    pub firm_id: Option<Uuid>,
    pub primary_color: Option<String>,
    pub accent_color: Option<String>,
    pub typeface: Option<String>,
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
    /// Only a Firm's own Admin DRI may create that Firm's brand. Every other
    /// actor, and an Admin DRI naming a different Firm, is refused this.
    #[error("you may not create, edit, or delete this brand")]
    NotAuthorized,
    /// [`create`] was called with `firm_id: None` (ENG-659). Every brand is
    /// Firm-scoped now — there is no system-wide brand an actor, Owner
    /// included, may create through this command.
    #[error("a brand must belong to a Firm")]
    FirmRequired,
    /// The proposed `primary_color` is not a well-formed `#rrggbb` hex.
    #[error("{0} is not a valid #rrggbb hex colour")]
    InvalidHex(String),
    /// The proposed `primary_color` fails one of the two fixed-background
    /// contrast checks (ENG-629): its deterministically-chosen on-primary
    /// text colour, or the light page surface it renders on. `against`
    /// names which one.
    #[error("{hex} is {ratio:.1}:1 against {against}; it must be at least {required:.1}:1")]
    InsufficientContrast {
        hex: String,
        against: &'static str,
        ratio: f64,
        required: f64,
    },
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
            Self::InsufficientContrast {
                hex,
                against,
                ratio,
                required,
            } => format!(
                "{hex} is {ratio:.1}:1 against {against}; it must be at least {required:.1}:1."
            ),
            Self::InvalidFontLicence(_) => {
                format!("Pick a font licence: {}.", FONT_LICENCES.join(", "))
            }
            Self::StillReferenced => {
                "That brand is still worn by a firm or named by a project.".to_string()
            }
            Self::NotAuthorized => "You may not create, edit, or delete this brand.".to_string(),
            Self::FirmRequired => "A brand must belong to a Firm.".to_string(),
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

/// Whether `actor` may create a brand at `target_firm_id`.
///
/// Side-effect-free. `target_firm_id: None` is refused outright with
/// [`BrandError::FirmRequired`] before any role check — every brand is
/// Firm-scoped (ENG-659), so there is no system-wide case left to authorize,
/// Owner included. Only that Firm's own Admin DRI passes; every other
/// combination, including that Firm's non-DRI Admin, is refused.
async fn authorize(
    surreal: &SurrealDb,
    actor_person_id: Option<Uuid>,
    target_firm_id: Option<Uuid>,
) -> Result<(), BrandError> {
    let Some(firm_id) = target_firm_id else {
        return Err(BrandError::FirmRequired);
    };
    if crate::firms::find_by_id(surreal, firm_id).await?.is_none() {
        return Err(BrandError::NoSuchFirm(firm_id));
    }
    let Some(person_id) = actor_person_id else {
        return Err(BrandError::NotAuthorized);
    };
    match crate::firms::membership_for_person(surreal, person_id, firm_id).await? {
        Some(row) if row.is_dri && row.membership == crate::firms::FirmMembership::Admin => Ok(()),
        _ => Err(BrandError::NotAuthorized),
    }
}

/// Authorize an edit to an existing brand. Owner governs existing brands on
/// every Firm; a Firm's own Admin DRI governs its Firm-scoped brand. A
/// Firm-scoped target routes through
/// [`crate::firm_capability::resolve_quietly`] with
/// [`crate::firm_capability::FirmCapability::ManageBrand`] (ENG-645) rather
/// than re-deriving the Owner-bypass-then-Admin-DRI rule by hand, so the two
/// can no longer drift; `resolve_quietly` is the non-emitting entry point
/// because this is a defense-in-depth check behind a command
/// [`find_by_key_for_actor`] already authorized once through the emitting
/// [`crate::firm_capability::resolve`]. The `None` arm below only ever
/// matches a historical `firm_id IS NONE` row that predates ENG-659's
/// backfill; [`create`] refuses to write a new one.
async fn authorize_existing(
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
            match crate::firm_capability::resolve_quietly(
                surreal,
                actor_role,
                actor_person_id,
                firm_id,
                crate::firm_capability::FirmCapability::ManageBrand,
            )
            .await?
            {
                crate::firm_capability::FirmCapabilityDecision::Allowed => Ok(()),
                crate::firm_capability::FirmCapabilityDecision::Forbidden => {
                    Err(BrandError::NotAuthorized)
                }
                crate::firm_capability::FirmCapabilityDecision::FirmNotFound => {
                    Err(BrandError::NoSuchFirm(firm_id))
                }
            }
        }
    }
}

/// Create a brand. See the module doc for the authorization rule and what a
/// Firm-scoped request inherits. `actor_role` is accepted for parity with
/// every other command in this module even though [`authorize`] no longer
/// branches on it — a Firm-scoped create is an Admin-DRI-only act now,
/// whatever the caller's role.
pub async fn create(
    surreal: &SurrealDb,
    _actor_role: Role,
    actor_person_id: Option<Uuid>,
    input: &NewBrand,
) -> Result<Brand, BrandError> {
    authorize(surreal, actor_person_id, input.firm_id).await?;
    create_unchecked(surreal, input).await
}

/// The write half of [`create`], with no authorization check. Shared with
/// [`seed_upsert`], the boot seed's trusted system path (ENG-659) — every
/// caller-driven path goes through [`create`], which calls [`authorize`]
/// first.
async fn create_unchecked(surreal: &SurrealDb, input: &NewBrand) -> Result<Brand, BrandError> {
    if let Some(hex) = input.primary_color.as_deref() {
        validate_primary_hex(hex)?;
    }

    // The caller already proved `input.firm_id` is `Some` and that Firm
    // exists — a Firm-scoped brand always inherits `is_law_firm = true` and
    // its Firm's own Entity name; neither is caller input.
    let firm_id = input.firm_id.ok_or(BrandError::FirmRequired)?;
    let firm = crate::firms::find_by_id(surreal, firm_id)
        .await?
        .ok_or(BrandError::NoSuchFirm(firm_id))?;
    let legal_entity = match firm.entity_id {
        Some(entity_id) => crate::entities::find_by_id(surreal, entity_id)
            .await
            .map_err(crate::firms::FirmError::from)?
            .map(|entity| entity.name),
        None => None,
    };
    let is_law_firm = true;

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

/// System/seed-only: create `input`, or, if its name or key already exists,
/// refresh only its `typeface` and `primary_color` — with no authorization
/// check (ENG-659). This is the boot seed's own migration of each compiled
/// house brand into a `brand` row scoped to the resolved practice Firm
/// (`store::seed::seed_brands`); it must succeed even against a test engine
/// where an unrelated fixture Firm already registered the same compiled keys
/// (`store::surreal::test_support::mem`), which the authorized [`create`]/
/// [`update`] pair would correctly refuse — this path is why the boot seed
/// does not need to be that caller. Every human- or API-driven path still
/// goes through the authorized [`create`]/[`update`].
pub(crate) async fn seed_upsert(surreal: &SurrealDb, input: &NewBrand) -> Result<Brand, BrandError> {
    match create_unchecked(surreal, input).await {
        Ok(brand) => Ok(brand),
        Err(BrandError::DuplicateName | BrandError::DuplicateKey) => {
            let existing = find_by_key(surreal, &input.key)
                .await?
                .ok_or_else(|| BrandError::NoSuchBrand(Uuid::nil()))?;
            update_unchecked(
                surreal,
                existing.id,
                &BrandEdit {
                    typeface: Some(input.typeface.clone()),
                    primary_color: Some(input.primary_color.clone()),
                    ..BrandEdit::default()
                },
            )
            .await
        }
        Err(error) => Err(error),
    }
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

/// Find a brand and resolve the caller's Firm capability before a caller uses
/// the row for a Firm-scoped read or command. System-wide brands remain
/// Owner-only; Firm-scoped brands go through [`crate::firm_capability::resolve`]
/// with [`crate::firm_capability::FirmCapability::ManageBrand`]. Missing and
/// forbidden targets stay typed so each response boundary can render the same
/// not-found answer without disclosing another Firm's row.
pub async fn find_by_key_for_actor(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
    key: &str,
) -> Result<Brand, BrandError> {
    let brand = find_by_key(surreal, key)
        .await?
        .ok_or_else(|| BrandError::NoSuchBrand(Uuid::nil()))?;
    let Some(firm_id) = brand.firm_id else {
        return if actor_role == Role::Owner {
            Ok(brand)
        } else {
            Err(BrandError::NotAuthorized)
        };
    };

    match crate::firm_capability::resolve(
        surreal,
        actor_role,
        actor_person_id,
        firm_id,
        crate::firm_capability::FirmCapability::ManageBrand,
    )
    .await?
    {
        crate::firm_capability::FirmCapabilityDecision::Allowed => Ok(brand),
        crate::firm_capability::FirmCapabilityDecision::Forbidden => Err(BrandError::NotAuthorized),
        crate::firm_capability::FirmCapabilityDecision::FirmNotFound => {
            Err(BrandError::NoSuchFirm(firm_id))
        }
    }
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
/// Owner inventory [`visible_for_actor`] lists alongside [`system_wide`]. Never
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

/// Brands the actor may see on `/app/admin/brands`.
///
/// System-wide rows are visible to every Firm. Owner also sees every
/// Firm-scoped row. An Admin sees only the Firm-scoped rows they hold
/// [`crate::firm_capability::FirmCapability::ManageBrand`] on — that Firm's
/// Admin DRI — so one practice's administrator cannot read another's
/// presentation inventory.
pub async fn visible_for_actor(
    surreal: &SurrealDb,
    actor_role: Role,
    actor_person_id: Option<Uuid>,
) -> Result<Vec<Brand>, BrandError> {
    let mut brands = match actor_role {
        Role::Owner => {
            let mut brands = system_wide(surreal).await?;
            brands.extend(all_firm_scoped(surreal).await?);
            brands
        }
        Role::Admin => {
            let mut brands = system_wide(surreal).await?;
            let firm_ids = crate::firm_capability::allowed_firm_ids(
                surreal,
                actor_role,
                actor_person_id,
                crate::firm_capability::FirmCapability::ManageBrand,
            )
            .await?;
            for firm_id in firm_ids {
                brands.extend(for_firm(surreal, firm_id).await?);
            }
            brands
        }
        Role::Lawyer | Role::Clerk | Role::Client => Vec::new(),
    };
    brands.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(brands)
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

/// The distinct font family names this Firm has actually uploaded, across
/// every brand row it owns — sorted and de-duplicated. This is the *only*
/// source the Admin edit page's typeface `<select>` draws from (ENG-659):
/// empty when the Firm has uploaded no font yet, so the compiled
/// `views::brand_presentation::TYPEFACES` catalog never appears there. A row
/// with `font_family` set but no `font_object_key` (named but never
/// actually uploaded) is excluded — the option only ever names a real
/// uploaded object.
pub async fn uploaded_font_families_for_firm(
    surreal: &SurrealDb,
    firm_id: Uuid,
) -> Result<Vec<String>, BrandError> {
    let mut families: Vec<String> = for_firm(surreal, firm_id)
        .await?
        .into_iter()
        .filter(|brand| brand.font_object_key.is_some())
        .filter_map(|brand| brand.font_family)
        .collect();
    families.sort();
    families.dedup();
    Ok(families)
}

/// Edit a brand's presentation fields. Owner governs existing brands on every
/// Firm; a Firm's own Admin DRI governs its Firm-scoped one.
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
    authorize_existing(surreal, actor_role, actor_person_id, existing.firm_id).await?;
    update_unchecked(surreal, brand_id, input).await
}

/// The write half of [`update`], with no authorization check. Shared with
/// [`seed_upsert`]; see [`create_unchecked`] for why the boot seed needs
/// this. `brand_id` must already have been resolved to an existing row by
/// the caller.
async fn update_unchecked(
    surreal: &SurrealDb,
    brand_id: Uuid,
    input: &BrandEdit,
) -> Result<Brand, BrandError> {
    let existing = find_by_id(surreal, brand_id)
        .await?
        .ok_or(BrandError::NoSuchBrand(brand_id))?;
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
    authorize_existing(surreal, actor_role, actor_person_id, existing.firm_id).await?;

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
    authorize_existing(surreal, actor_role, actor_person_id, existing.firm_id).await?;

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
    authorize_existing(surreal, actor_role, actor_person_id, existing.firm_id).await?;
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

    /// ENG-659: every brand is Firm-scoped now. `create` refuses
    /// `firm_id: None` before any role check, for every actor including
    /// Owner — there is no system-wide brand left to create.
    #[tokio::test]
    async fn create_refuses_a_missing_firm_id_for_every_actor() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Any Practice").await;

        let owner_err = create(
            &db,
            Role::Owner,
            None,
            &NewBrand {
                name: "Acme Law".to_string(),
                key: "acme-law".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(owner_err, BrandError::FirmRequired));

        let admin_err = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Admin Attempt".to_string(),
                key: "admin-attempt".to_string(),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(admin_err, BrandError::FirmRequired));

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
        assert!(matches!(err, BrandError::FirmRequired));

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
                    firm_id: Some(firm_a.id),
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
        let (firm, admin) = practice(&db, "Uniqueness Practice").await;
        create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "First".to_string(),
                key: "first".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();

        let duplicate_name = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "First".to_string(),
                key: "second".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(duplicate_name, BrandError::DuplicateName));

        let duplicate_key = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Second".to_string(),
                key: "first".to_string(),
                firm_id: Some(firm.id),
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

        let owner_edited = update(
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
        .unwrap();
        assert_eq!(owner_edited.name, "Hijacked");

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
        assert_eq!(edited.name, "Hijacked");
        assert_eq!(edited.primary_color.as_deref(), Some("#123456"));
        assert_eq!(edited.firm_id, Some(firm.id));

        delete(&db, Role::Owner, None, brand.id).await.unwrap();
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
        let (firm, admin) = practice(&db, "Hex Practice").await;

        let err = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Malformed Hex".to_string(),
                key: "malformed-hex".to_string(),
                firm_id: Some(firm.id),
                primary_color: Some("not-a-hex".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::InvalidHex(hex) if hex == "not-a-hex"));

        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Valid Hex".to_string(),
                key: "valid-hex".to_string(),
                firm_id: Some(firm.id),
                primary_color: Some("#007c91".to_string()),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(brand.primary_color.as_deref(), Some("#007c91"));

        let err = update(
            &db,
            Role::Admin,
            Some(admin),
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

    /// ENG-629: the old "best of white or black" gate had a mathematical
    /// floor of `sqrt(1.05 * 0.05) / 0.05 ≈ 4.58` for every well-formed hex,
    /// so it could never reject one — a pale yellow or near-white primary
    /// read as "low contrast" against the white page around it while still
    /// clearing the gate against black text. Checking the primary against
    /// [`LIGHT_PAGE_SURFACE`] (what the page actually renders it on) instead
    /// of the best of two self-selected extremes is what makes those two
    /// colours refusable, while a genuinely darker or more saturated primary
    /// — which does contrast against the real white page — still clears
    /// both checks exactly as before.
    #[tokio::test]
    async fn create_and_update_refuse_a_pale_primary_that_would_clear_the_old_best_of_gate() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Contrast Practice").await;
        for (key, hex) in [("near-black", "#010101"), ("mid-gray", "#808080")] {
            let brand = create(
                &db,
                Role::Admin,
                Some(admin),
                &NewBrand {
                    name: key.to_string(),
                    key: key.to_string(),
                    firm_id: Some(firm.id),
                    primary_color: Some(hex.to_string()),
                    ..NewBrand::default()
                },
            )
            .await
            .unwrap_or_else(|error| {
                panic!("{hex} must still clear the fixed-background gate: {error}")
            });
            assert_eq!(brand.primary_color.as_deref(), Some(hex));
        }

        for (key, hex) in [("pale-yellow", "#f5f5a0"), ("near-white", "#fefefe")] {
            let err = create(
                &db,
                Role::Admin,
                Some(admin),
                &NewBrand {
                    name: key.to_string(),
                    key: key.to_string(),
                    firm_id: Some(firm.id),
                    primary_color: Some(hex.to_string()),
                    ..NewBrand::default()
                },
            )
            .await
            .unwrap_err();
            assert!(
                matches!(
                    &err,
                    BrandError::InsufficientContrast { against, .. } if *against == "the light page surface"
                ),
                "{hex}: {err}"
            );
        }

        // `update` refuses the same pale primary on an already-created brand.
        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Editable".to_string(),
                key: "editable-pale".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        let err = update(
            &db,
            Role::Admin,
            Some(admin),
            brand.id,
            &BrandEdit {
                primary_color: Some(Some("#fefefe".to_string())),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, BrandError::InsufficientContrast { .. }));
    }

    /// ENG-586: `set_font` refuses a licence outside the closed list before
    /// any write, and stores a valid one alongside the family and object key.
    #[tokio::test]
    async fn set_font_validates_the_licence_and_stores_the_upload() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Fontable Practice").await;
        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Fontable".to_string(),
                key: "fontable".to_string(),
                firm_id: Some(firm.id),
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

    /// ENG-659: the Admin edit page's typeface select draws only from a
    /// Firm's own uploaded fonts — empty before any upload, and it never
    /// lists a compiled catalog id. A brand naming a `font_family` but
    /// never actually uploading an object (no `font_object_key`) is
    /// excluded, and another Firm's upload never leaks in.
    #[tokio::test]
    async fn uploaded_font_families_for_firm_lists_only_this_firm_s_real_uploads() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Font Roster Practice").await;
        let (other_firm, other_admin) = practice(&db, "Other Font Practice").await;

        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Font Roster Brand".to_string(),
                key: "font-roster-brand".to_string(),
                firm_id: Some(firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert!(uploaded_font_families_for_firm(&db, firm.id)
            .await
            .unwrap()
            .is_empty());

        // Named but never uploaded — `font_object_key` stays unset, so this
        // must not appear as a selectable option.
        update(
            &db,
            Role::Admin,
            Some(admin),
            brand.id,
            &BrandEdit {
                font_family: Some(Some("Named Only".to_string())),
                ..BrandEdit::default()
            },
        )
        .await
        .unwrap();
        assert!(uploaded_font_families_for_firm(&db, firm.id)
            .await
            .unwrap()
            .is_empty());

        set_font(
            &db,
            Role::Admin,
            Some(admin),
            brand.id,
            "Custom Sans",
            "fonts/brands/font-roster-brand/abc123.woff2",
            "OFL-1.1",
        )
        .await
        .unwrap();
        assert_eq!(
            uploaded_font_families_for_firm(&db, firm.id).await.unwrap(),
            vec!["Custom Sans".to_string()]
        );

        create(
            &db,
            Role::Admin,
            Some(other_admin),
            &NewBrand {
                name: "Other Font Brand".to_string(),
                key: "other-font-brand".to_string(),
                firm_id: Some(other_firm.id),
                ..NewBrand::default()
            },
        )
        .await
        .unwrap();
        assert!(uploaded_font_families_for_firm(&db, other_firm.id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            uploaded_font_families_for_firm(&db, firm.id).await.unwrap(),
            vec!["Custom Sans".to_string()],
            "another Firm's upload must not leak in"
        );
    }

    /// ENG-586: `set_logo` is authorized exactly like `update`, and stores the
    /// object key and content type.
    #[tokio::test]
    async fn set_logo_stores_the_object_key_and_content_type() {
        let db = mem_surreal().await;
        let (firm, admin) = practice(&db, "Logoable Practice").await;
        let brand = create(
            &db,
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Logoable".to_string(),
                key: "logoable".to_string(),
                firm_id: Some(firm.id),
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
    /// every Firm, distinct from `for_firm`'s single-Firm scope. Also covers
    /// ENG-659's read-side tolerance: a historical `firm_id IS NONE` row (no
    /// live path creates one any more) still surfaces in `system_wide` and
    /// every actor's `visible_for_actor` view.
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
        // ENG-659: there is no live path left to create a `firm_id: None`
        // row, so a historical one is simulated with a raw write, exactly
        // as a row a pre-ENG-659 build once created (and this build's
        // schema backfill has not yet visited) would look on disk. The
        // schema itself already tightened `firm_id` to a required
        // `record<firm>` the moment `mem_surreal` applied it (there were no
        // orphans yet to backfill), so this loosens it back open first —
        // the same trick `store::schema::mod::tests::historical_orphan_brand`
        // uses for exactly this reason.
        db.query("DEFINE FIELD OVERWRITE firm_id ON brand TYPE option<record<firm>>")
            .await
            .unwrap()
            .check()
            .unwrap();
        let system_wide_id = Uuid::now_v7();
        let now = chrono::Utc::now().to_rfc3339();
        db.query(format!(
            "CREATE $id SET name = 'System Wide', brand_key = 'system-wide', \
             is_law_firm = false, inserted_at = $now, updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, system_wide_id)))
        .bind(("now", now))
        .await
        .unwrap()
        .check()
        .unwrap();
        let system_wide_brand = find_by_id(&db, system_wide_id).await.unwrap().unwrap();
        assert_eq!(system_wide_brand.firm_id, None);

        // `mem_surreal`'s own fixture already pre-seeds every compiled
        // house-brand key as a Firm-scoped row (ENG-659), so `all_firm_scoped`
        // returns those too — this only asserts `a` and `b` are among them,
        // not that they are the whole set.
        let all = all_firm_scoped(&db).await.unwrap();
        assert!(all.contains(&a));
        assert!(all.contains(&b));
        assert!(!all.contains(&system_wide_brand));

        let owner_view = visible_for_actor(&db, Role::Owner, None).await.unwrap();
        assert!(owner_view.contains(&a));
        assert!(owner_view.contains(&b));
        assert!(owner_view.contains(&system_wide_brand));

        let first_practice = visible_for_actor(&db, Role::Admin, Some(admin_a))
            .await
            .unwrap();
        assert!(first_practice.contains(&a));
        assert!(first_practice.contains(&system_wide_brand));
        assert!(!first_practice.contains(&b));

        let second_practice = visible_for_actor(&db, Role::Admin, Some(admin_b))
            .await
            .unwrap();
        assert!(second_practice.contains(&b));
        assert!(second_practice.contains(&system_wide_brand));
        assert!(!second_practice.contains(&a));
    }

    /// ENG-586: deleting a brand a Firm wears, or that a Project names, is
    /// refused; deleting an unworn, unnamed brand removes the row. Uses
    /// runtime-created keys rather than the nine compiled ones: ENG-587
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
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Worn Brand".to_string(),
                key: "worn-brand".to_string(),
                firm_id: Some(firm.id),
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
            Role::Admin,
            Some(admin),
            &NewBrand {
                name: "Named Brand".to_string(),
                key: "named-brand".to_string(),
                firm_id: Some(firm.id),
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

    async fn person_with_membership(
        db: &SurrealDb,
        firm_id: Uuid,
        role: Role,
        membership: Option<(FirmMembership, bool)>,
    ) -> Uuid {
        let person_id = crate::persons::create(
            db,
            &NewPerson::with_role(
                "Resolver Parity Person",
                format!("resolver-parity-{}@example.com", Uuid::now_v7()),
                role,
            ),
        )
        .await
        .unwrap()
        .id;
        if let Some((membership, is_dri)) = membership {
            crate::firms::add_membership(
                db,
                &NewPersonFirmRole {
                    person_id,
                    firm_id,
                    membership,
                    is_dri,
                },
            )
            .await
            .unwrap();
        }
        person_id
    }

    /// ENG-645: `authorize_existing`'s Firm-scoped decision must never drift
    /// from `firm_capability::resolve(ManageBrand)`, the check it now calls
    /// (through the non-emitting [`crate::firm_capability::resolve_quietly`])
    /// instead of restating by hand. Sweeps Owner, Client, and every
    /// Admin/Lawyer/Clerk actor across every membership tier and DRI
    /// combination, plus a target Firm that does not exist, and asserts the
    /// two always agree.
    #[tokio::test]
    async fn authorize_existing_agrees_with_the_capability_resolver_for_every_combination() {
        use crate::firm_capability::{FirmCapability, FirmCapabilityDecision};

        let db = mem_surreal().await;
        let (firm, admin_dri) = practice(&db, "Resolver Parity Practice").await;

        let mut cases: Vec<(Role, Option<Uuid>)> = vec![
            (Role::Owner, None),
            (Role::Client, None),
            // Seeded by `practice`: Admin membership with `is_dri = true`.
            (Role::Admin, Some(admin_dri)),
        ];
        for role in [Role::Admin, Role::Lawyer, Role::Clerk] {
            // No `person_firm_role` row at all.
            cases.push((role, None));
            for membership in [
                FirmMembership::Admin,
                FirmMembership::Lawyer,
                FirmMembership::Clerk,
            ] {
                for is_dri in [true, false] {
                    let person_id =
                        person_with_membership(&db, firm.id, role, Some((membership, is_dri)))
                            .await;
                    cases.push((role, Some(person_id)));
                }
            }
        }

        for (role, person_id) in cases {
            let expected = crate::firm_capability::resolve(
                &db,
                role,
                person_id,
                firm.id,
                FirmCapability::ManageBrand,
            )
            .await
            .unwrap();
            let actual = authorize_existing(&db, role, person_id, Some(firm.id)).await;
            match expected {
                FirmCapabilityDecision::Allowed => {
                    assert!(actual.is_ok(), "{role:?}/{person_id:?} expected allowed");
                }
                FirmCapabilityDecision::Forbidden => {
                    assert!(
                        matches!(actual, Err(BrandError::NotAuthorized)),
                        "{role:?}/{person_id:?} expected NotAuthorized, got {actual:?}"
                    );
                }
                FirmCapabilityDecision::FirmNotFound => {
                    assert!(
                        matches!(actual, Err(BrandError::NoSuchFirm(_))),
                        "{role:?}/{person_id:?} expected NoSuchFirm, got {actual:?}"
                    );
                }
            }
        }

        // A Firm that does not exist agrees too.
        let missing_firm = Uuid::now_v7();
        let expected = crate::firm_capability::resolve(
            &db,
            Role::Owner,
            None,
            missing_firm,
            FirmCapability::ManageBrand,
        )
        .await
        .unwrap();
        assert_eq!(expected, FirmCapabilityDecision::FirmNotFound);
        let actual = authorize_existing(&db, Role::Owner, None, Some(missing_firm)).await;
        assert!(matches!(actual, Err(BrandError::NoSuchFirm(id)) if id == missing_firm));
    }
}
