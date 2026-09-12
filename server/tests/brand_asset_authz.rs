//! Firm-scoped brand asset writes authorize before touching public storage.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use portal::session::{SessionData, SessionStore, SESSION_COOKIE_NAME};
use store::firms::{FirmMembership, NewFirm};
use store::persons::{NewPerson, Role};
use store::test_support::{mem_surreal, seed_entity};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const SESSION_KEY: &str = "test-session-key-not-for-production";
const BRAND_KEY: &str = "practice-b-brand";
const LOGO_KEY: &str = "brands/practice-b-brand/logo.png";
const ORIGINAL_LOGO: &[u8] = b"original-logo";
const REPLACEMENT_LOGO: &[u8] = b"replacement-logo";
const ORIGINAL_FONT: &[u8] = b"original-font";
const REPLACEMENT_FONT: &[u8] = b"replacement-font";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    storage: Arc<dyn cloud::StorageService>,
    _storage_root: TempDir,
    admin_a: SessionCookie,
    admin_b: SessionCookie,
    owner: SessionCookie,
}

struct SessionCookie {
    cookie: String,
    csrf: String,
}

struct SeededBrand {
    id: Uuid,
    font_key: String,
}

async fn build() -> (Fixture, SeededBrand) {
    let surreal = mem_surreal().await;
    let (_firm_a, unassigned_firm_admin_id) = firm(&surreal, "Practice A").await;
    let (firm_b, target_firm_admin_id) = firm(&surreal, "Practice B").await;
    let owner = store::persons::create(
        &surreal,
        &NewPerson::with_role("Owner", "owner@example.com", Role::Owner),
    )
    .await
    .unwrap();
    let brand = store::brands::create(
        &surreal,
        Role::Admin,
        Some(target_firm_admin_id),
        &store::brands::NewBrand {
            name: "Practice B Brand".to_string(),
            key: BRAND_KEY.to_string(),
            firm_id: Some(firm_b.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let replacement_font_key = format!(
        "fonts/brands/{BRAND_KEY}/{}.woff2",
        store::assets::sha256_hex(REPLACEMENT_FONT)
    );
    let storage_root = TempDir::new().unwrap();
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(storage_root.path().to_path_buf())
            .await
            .unwrap(),
    );
    storage
        .put(LOGO_KEY, ORIGINAL_LOGO, "image/png")
        .await
        .unwrap();
    storage
        .put(&replacement_font_key, ORIGINAL_FONT, "font/woff2")
        .await
        .unwrap();
    store::brands::set_logo(
        &surreal,
        Role::Admin,
        Some(target_firm_admin_id),
        brand.id,
        LOGO_KEY,
        "image/png",
    )
    .await
    .unwrap();
    store::brands::set_font(
        &surreal,
        Role::Admin,
        Some(target_firm_admin_id),
        brand.id,
        "Original Sans",
        &replacement_font_key,
        "OFL-1.1",
    )
    .await
    .unwrap();

    let mut state = portal::test_support::app_state(surreal.clone()).await;
    state.storage = storage.clone();
    state.assets_storage = storage.clone();
    let sessions = SessionStore::new(SESSION_KEY);
    state.sessions = sessions.clone();
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let (admin_a, _) = session(unassigned_firm_admin_id, Role::Admin);
    let (admin_b, _) = session(target_firm_admin_id, Role::Admin);
    let (owner, _) = session(owner.id, Role::Owner);
    (
        Fixture {
            app,
            surreal,
            storage,
            _storage_root: storage_root,
            admin_a,
            admin_b,
            owner,
        },
        SeededBrand {
            id: brand.id,
            font_key: replacement_font_key,
        },
    )
}

async fn firm(db: &store::surreal::SurrealDb, name: &str) -> (store::firms::Firm, Uuid) {
    let entity_id = seed_entity(db).await;
    let admin = store::persons::create(
        db,
        &NewPerson::with_role(
            format!("{name} Admin"),
            format!(
                "{}@example.com",
                name.to_ascii_lowercase().replace(' ', "-")
            ),
            Role::Admin,
        ),
    )
    .await
    .unwrap();
    let firm = store::firms::create(
        db,
        &NewFirm {
            name: name.to_string(),
            status: "active".to_string(),
            entity_id,
            admin_dri_person_id: admin.id,
        },
    )
    .await
    .unwrap();
    let membership = store::firms::membership_for_person(db, admin.id, firm.id)
        .await
        .unwrap();
    assert!(matches!(
        membership,
        Some(row) if row.membership == FirmMembership::Admin && row.is_dri
    ));
    (firm, admin.id)
}

fn session(person_id: Uuid, role: Role) -> (SessionCookie, SessionData) {
    let mut data = SessionData::fresh(person_id.to_string(), role);
    data.person_id = Some(person_id);
    let sessions = SessionStore::new(SESSION_KEY);
    (
        SessionCookie {
            cookie: format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&data)),
            csrf: data.csrf_token.clone(),
        },
        data,
    )
}

fn multipart_body(
    boundary: &str,
    csrf: &str,
    fields: &[(&str, &str)],
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"_csrf\"\r\n\r\n");
    body.extend_from_slice(csrf.as_bytes());
    for (name, value) in fields {
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
    }
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn post_logo(fixture: &Fixture, session: &SessionCookie, bytes: &[u8]) -> StatusCode {
    let boundary = "----navigator-brand-logo-boundary";
    fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/brands/{BRAND_KEY}/logo"))
                .header(header::COOKIE, &session.cookie)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_body(
                    boundary,
                    &session.csrf,
                    &[],
                    "logo.png",
                    "image/png",
                    bytes,
                )))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn post_font(fixture: &Fixture, session: &SessionCookie, bytes: &[u8]) -> StatusCode {
    let boundary = "----navigator-brand-font-boundary";
    fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/brands/{BRAND_KEY}/font"))
                .header(header::COOKIE, &session.cookie)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_body(
                    boundary,
                    &session.csrf,
                    &[("family", "Replacement Sans"), ("licence", "OFL-1.1")],
                    "replacement.woff2",
                    "font/woff2",
                    bytes,
                )))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn post_presentation(fixture: &Fixture, session: &SessionCookie) -> StatusCode {
    let body = format!(
        "_csrf={}&typeface=gorp-serif&primary_color=%23007c91&font_family=Replacement+Sans",
        session.csrf
    );
    fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/brands/{BRAND_KEY}/edit"))
                .header(header::COOKIE, &session.cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn get_edit(fixture: &Fixture, session: &SessionCookie) -> StatusCode {
    fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/app/brands/{BRAND_KEY}/edit"))
                .header(header::COOKIE, &session.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn an_out_of_scope_admin_cannot_replace_brand_assets_or_presentation() {
    let (fixture, brand) = build().await;

    assert_eq!(
        post_logo(&fixture, &fixture.admin_a, REPLACEMENT_LOGO).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post_font(&fixture, &fixture.admin_a, REPLACEMENT_FONT).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_edit(&fixture, &fixture.admin_a).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post_presentation(&fixture, &fixture.admin_a).await,
        StatusCode::NOT_FOUND
    );

    let logo = fixture.storage.get(LOGO_KEY).await.unwrap();
    assert_eq!(logo.bytes, ORIGINAL_LOGO);
    let font = fixture.storage.get(&brand.font_key).await.unwrap();
    assert_eq!(font.bytes, ORIGINAL_FONT);
    let unchanged = store::brands::find_by_id(&fixture.surreal, brand.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.logo_object_key.as_deref(), Some(LOGO_KEY));
    assert_eq!(unchanged.logo_content_type.as_deref(), Some("image/png"));
    assert_eq!(
        unchanged.font_object_key.as_deref(),
        Some(brand.font_key.as_str())
    );
    assert_eq!(unchanged.font_family.as_deref(), Some("Original Sans"));
    assert_eq!(unchanged.font_licence.as_deref(), Some("OFL-1.1"));
}

#[tokio::test]
async fn the_target_admin_and_owner_can_update_brand_assets() {
    let (fixture, brand) = build().await;

    assert_eq!(
        post_logo(&fixture, &fixture.admin_b, REPLACEMENT_LOGO).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_font(&fixture, &fixture.admin_b, REPLACEMENT_FONT).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        fixture.storage.get(LOGO_KEY).await.unwrap().bytes,
        REPLACEMENT_LOGO
    );
    assert_eq!(
        fixture.storage.get(&brand.font_key).await.unwrap().bytes,
        REPLACEMENT_FONT
    );

    let owner_logo = b"owner-logo";
    let owner_font = b"owner-font";
    assert_eq!(
        post_logo(&fixture, &fixture.owner, owner_logo).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post_font(&fixture, &fixture.owner, owner_font).await,
        StatusCode::SEE_OTHER
    );
    let owner_font_key = format!(
        "fonts/brands/{BRAND_KEY}/{}.woff2",
        store::assets::sha256_hex(owner_font)
    );
    assert_eq!(
        fixture.storage.get(LOGO_KEY).await.unwrap().bytes,
        owner_logo
    );
    assert_eq!(
        fixture.storage.get(&owner_font_key).await.unwrap().bytes,
        owner_font
    );
    let updated = store::brands::find_by_id(&fixture.surreal, brand.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.logo_object_key.as_deref(), Some(LOGO_KEY));
    assert_eq!(
        updated.font_object_key.as_deref(),
        Some(owner_font_key.as_str())
    );
    assert_eq!(updated.font_family.as_deref(), Some("Replacement Sans"));
    assert_eq!(updated.font_licence.as_deref(), Some("OFL-1.1"));
}

/// The negative case above proves an out-of-scope Admin is refused on the edit
/// GET and the presentation POST. Both doors now answer from
/// `store::brands::find_by_key_for_actor`, so a resolver that refused
/// *everyone* would satisfy that assertion just as well. Pin the other side of
/// the boundary: the target Firm's Admin DRI and Owner still read the editor
/// and still write presentation through the same lookup.
#[tokio::test]
async fn the_target_admin_and_owner_can_read_and_edit_brand_presentation() {
    let (fixture, brand) = build().await;

    assert_eq!(get_edit(&fixture, &fixture.admin_b).await, StatusCode::OK);
    assert_eq!(get_edit(&fixture, &fixture.owner).await, StatusCode::OK);

    assert_eq!(
        post_presentation(&fixture, &fixture.admin_b).await,
        StatusCode::SEE_OTHER
    );
    let edited = store::brands::find_by_id(&fixture.surreal, brand.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(edited.typeface.as_deref(), Some("gorp-serif"));
    assert_eq!(edited.primary_color.as_deref(), Some("#007c91"));
    assert_eq!(edited.font_family.as_deref(), Some("Replacement Sans"));

    assert_eq!(
        post_presentation(&fixture, &fixture.owner).await,
        StatusCode::SEE_OTHER
    );
}
