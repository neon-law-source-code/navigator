//! `GET /app/projects/{code}/portal` — one client portal per Project.
//!
//! Every Project has exactly one front-end application: the client's portal,
//! built from that Project's own repository under `portal/` and mounted at this
//! fixed path. There is no `{application}` path parameter, no application name,
//! and no registry lookup that resolves one — the segment is the literal
//! [`cloud::workspace::PORTAL_MOUNT_SEGMENT`].
//!
//! # Why the extra segment exists
//!
//! Mounting the portal at `/app/projects/{code}` directly would shadow
//! Navigator's own matter show page, which is served at `/app/projects/{code}`.
//! The `portal` segment is what keeps that page, and it is the first thing
//! [`tests`] asserts.
//!
//! # Handler order
//!
//! 1. **Authenticate.** The route rides `session_boundary` like every other
//!    `/app` surface, so an anonymous caller is bounced to login and never
//!    reaches this module.
//! 2. **Resolve the code to a Project.**
//! 3. **Authorize through Project participation**, and answer a scope miss with
//!    404.
//! 4. **Stream that Project's published portal bundle** from the private
//!    applications bucket.
//!
//! # The bundle is streamed, never redirected
//!
//! The bytes are streamed through this handler from
//! [`AppState::applications_storage`](crate::AppState) — the private,
//! per-deployment `<project>-applications` bucket. A signed-URL redirect is
//! deliberately refused: it would be bearer-shareable for its lifetime with no
//! participation recheck, and it points at a different origin, so the session
//! cookie the bundle needs for its own `/app/api` reads would not travel.
//! Same-origin streaming is the whole mechanism that keeps the bundle
//! participation-gated.
//!
//! The mount serves like any single-page application:
//!
//! * The bare mount `301`s to the trailing-slash form, because a Vite base
//!   joins asset URLs directly onto it.
//! * A published object is streamed with its extension's content type. A
//!   content-hashed asset is immutable for a year; an `index.html` is
//!   `no-store`.
//! * A path with no published object of its own resolves to its directory
//!   index, then to that portal's `index.html` — so a multi-page build serves
//!   its own pages, and a single-bundle build's client-side route and a
//!   browser refresh both survive.
//! * A third-party bundle cannot ride Navigator's nonce CSP, so the response
//!   carries its own [`PORTAL_CSP`] instead.
//! * Every entrypoint (`index.html`, wherever it sits) has a small Neon Law
//!   banner spliced into it — see [`portal_banner_html`] — so a participant
//!   embedded in the Project's own bundle still has a way back to
//!   `/app/projects/{code}`. Every other object streams unmodified.
//!
//! # A scope miss is 404, never 403
//!
//! 403 would confirm that a Project with this code exists to somebody who is
//! not on it. Every refusal below — no such Project, no participation, nothing
//! published — is the same non-disclosing response, so the status code carries
//! no information about which one it was.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use cloud::workspace::PORTAL_MOUNT_SEGMENT;
use webapp::html_escape::escape_attr;

use crate::session::SessionData;

/// The one path a Project's client portal is served at.
///
/// `{code}` is the Project code, and `portal` is a literal rather than a
/// parameter. `/app/projects/{code}` stays Navigator's matter show page.
pub const PROJECT_PORTAL_PATH: &str = "/app/projects/{project_code}/portal";

/// The trailing-slash form the bare mount redirects to, and the root every
/// asset URL is joined onto by the Vite base.
const PROJECT_PORTAL_ROOT: &str = "/app/projects/{project_code}/portal/";

/// One published object below the mount. `{asset}` is the path within the
/// bundle; the wildcard is what makes a nested `assets/index-<hash>.js`
/// resolve.
const PROJECT_PORTAL_ASSET: &str = "/app/projects/{project_code}/portal/{*asset}";

/// The bundle entrypoint, served for the bare mount and for any unmatched
/// path below it.
const INDEX: &str = "index.html";

/// Content-hashed assets never change under their name, so they cache for a
/// year and are never revalidated. Distinct from the public assets lane's
/// `STATIC_CACHE_CONTROL`: this bundle is participation-gated, so it is
/// `private` rather than shared-cacheable.
const IMMUTABLE_CACHE: HeaderValue =
    HeaderValue::from_static("private, max-age=31536000, immutable");

/// `index.html` names the live build, so it must never be cached — a stale
/// copy would keep pointing at hashed assets a later publish has aged out.
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// A third-party Vite bundle cannot carry Navigator's per-request script
/// nonce, so the `/app/projects/{code}/portal` scope gets its own policy
/// rather than inheriting the nonce CSP. It is applied on the response, and
/// the global `if_not_present` CSP layer leaves it in place. `connect-src
/// 'self'` is what lets the bundle reach its own same-origin `/app/api`.
/// `style-src 'self' 'unsafe-inline'` is also what lets [`portal_banner_html`]
/// carry inline styles: the banner is spliced into the bundle's own
/// `index.html` rather than loaded from a stylesheet the bundle never links.
const PORTAL_CSP: HeaderValue = HeaderValue::from_static(
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; \
     object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
);

/// The handler's state: the store to resolve and authorize the Project, and
/// the applications bucket to stream its bundle from.
#[derive(Clone)]
struct PortalState {
    surreal: store::surreal::SurrealDb,
    applications: Arc<dyn cloud::StorageService>,
}

/// Mount the Project portal route.
///
/// The policy and auth layers match every other `/app` surface. Participation
/// authorization is the handler's own work, because it is per-Project rather
/// than per-tier.
pub fn router(
    surreal: store::surreal::SurrealDb,
    applications: Arc<dyn cloud::StorageService>,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    Router::new()
        .route(PROJECT_PORTAL_PATH, get(redirect_to_slash))
        .route(PROJECT_PORTAL_ROOT, get(serve_index))
        .route(PROJECT_PORTAL_ASSET, get(serve_asset))
        .with_state(PortalState {
            surreal,
            applications,
        })
        .route_layer(axum::middleware::from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::require_auth,
        ))
}

/// The one non-disclosing refusal.
///
/// Deliberately one function rather than a status code written at each refusal:
/// a reviewer can see that no branch below distinguishes "no such Project" from
/// "not your Project" from "nothing published yet".
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

/// Which gate closed on a portal request.
///
/// Every one of these returns the same non-disclosing 404, and that stays
/// deliberate: a caller must not be able to tell "no such Project" from "not
/// your Project". But the *operator* needs the distinction, and until deploy run
/// 32102608866 there was nowhere to read it — the browser fixture failed three
/// times against a 404 that named no gate, and the pod logs held only boot
/// output. Naming the branch in a log costs the caller nothing and is the
/// difference between a one-line answer and an afternoon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// An authenticated route reached with no session extension.
    NoSession,
    /// The code cannot name a Project, so the store is never asked.
    MalformedCode,
    /// A well-formed code no Project carries.
    NoSuchProject,
    /// A real Project the caller is not on. The participation ledger decides,
    /// with no Owner/Admin bypass.
    NotParticipating,
    /// A traversal or otherwise unsafe path within the bundle.
    UnsafeAssetPath,
    /// Nothing is published for this Project — not even the entrypoint the
    /// unmatched-path fallback looks for.
    NothingPublished,
}

impl Refused {
    /// The phrase logged for this gate. Distinct per variant, which
    /// `every_refusal_names_a_distinct_closed_gate` holds to.
    const fn reason(self) -> &'static str {
        match self {
            Self::NoSession => "no session on an authenticated route",
            Self::MalformedCode => "code cannot name a Project",
            Self::NoSuchProject => "no Project carries this code",
            Self::NotParticipating => "caller does not participate in this Project",
            Self::UnsafeAssetPath => "unsafe asset path",
            Self::NothingPublished => "no bundle published for this Project",
        }
    }
}

/// Refuse a portal request, saying which gate closed.
///
/// `warn`, not `debug`: the portal link is rendered only to viewers who already
/// pass this same gate, so a refusal is genuinely unexpected rather than routine
/// traffic — and a level below the pod's `INFO` would not be recorded at all,
/// which is exactly the hole this closes.
fn refuse(code: &str, asset: &str, refused: Refused) -> Response {
    tracing::warn!(
        project_code = %code,
        asset = %asset,
        reason = refused.reason(),
        "portal bundle refused"
    );
    not_found()
}

/// The bare mount carries no bundle path, so it redirects to the slashed root.
///
/// Unconditional and pre-authorization: appending a slash discloses nothing
/// about whether the Project exists or who participates. A `301` — the
/// conventional slash-normalization status — rather than `308`, because the
/// mount is `GET`-only and the redirect need not preserve a method.
async fn redirect_to_slash(Path(code): Path<String>) -> Response {
    let target = format!("/app/projects/{code}/portal/");
    match HeaderValue::from_str(&target) {
        Ok(location) => (
            StatusCode::MOVED_PERMANENTLY,
            [(header::LOCATION, location)],
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

/// The slashed root serves the bundle entrypoint.
async fn serve_index(
    State(state): State<PortalState>,
    session: Option<Extension<SessionData>>,
    Path(code): Path<String>,
) -> Response {
    serve_bundle(&state, session, &code, String::new()).await
}

/// One asset below the mount.
async fn serve_asset(
    State(state): State<PortalState>,
    session: Option<Extension<SessionData>>,
    Path((code, asset)): Path<(String, String)>,
) -> Response {
    serve_bundle(&state, session, &code, asset).await
}

/// Resolve, authorize, and stream one object from a Project's portal bundle.
async fn serve_bundle(
    state: &PortalState,
    session: Option<Extension<SessionData>>,
    code: &str,
    asset: String,
) -> Response {
    // 1. Authenticate. `session_boundary` has already bounced an anonymous
    //    caller; a request that arrives with no session extension anyway is
    //    refused rather than treated as a participant.
    let Some(Extension(session)) = session else {
        return refuse(code, &asset, Refused::NoSession);
    };

    // 2. Resolve the code. A malformed code cannot name a Project, so it is
    //    refused before the store is asked — `new` among them, which is the
    //    matter-open form rather than a Project.
    if !store::projects::is_valid_code(code) {
        return refuse(code, &asset, Refused::MalformedCode);
    }
    let Ok(Some(project)) = store::projects::find_by_code(&state.surreal, code).await else {
        return refuse(code, &asset, Refused::NoSuchProject);
    };

    // 3. Authorize through Project participation. `can_see_project` reads the
    //    participation ledger and carries **no** Owner/Admin project-scoping
    //    bypass, which is the `/app` rule: reaching a matter's surface means
    //    being on that matter.
    let participates =
        store::access::can_see_project(&state.surreal, session.person_id, session.role, project.id)
            .await
            .unwrap_or(false);
    if !participates {
        return refuse(code, &asset, Refused::NotParticipating);
    }

    // A traversal or otherwise-unsafe path cannot name a bundle object.
    if !asset_path_is_safe(&asset) {
        return refuse(code, &asset, Refused::UnsafeAssetPath);
    }

    // 4. Stream the object, walking the resolution order in
    //    `bundle_candidates`: the path itself, its directory index, then the
    //    entrypoint. A published page therefore serves itself, and a path
    //    nothing was published for still renders — so a client-side route and
    //    a refresh both survive.
    let prefix = format!("{code}/{PORTAL_MOUNT_SEGMENT}");
    for candidate in bundle_candidates(&asset) {
        match fetch(&state.applications, &format!("{prefix}/{candidate}")).await {
            Fetched::Found(object) => return bundle_response(&candidate, object, code),
            Fetched::Failed => return StatusCode::BAD_GATEWAY.into_response(),
            // Not this one; the next candidate is the point of the list.
            Fetched::Missing => {}
        }
    }

    // Not even the entrypoint is there, so nothing is published for this
    // Project — the same non-disclosing 404 a nonparticipant receives.
    refuse(code, &asset, Refused::NothingPublished)
}

/// The bundle-relative paths one request resolves against, in order.
///
/// Written as a list rather than nested in the fetch so the order is stated
/// once and can be asserted without a storage backend:
///
/// 1. **The path itself**, when it can name an object. A path ending in `/`
///    names none — no publish writes a key with a trailing slash — so it is
///    skipped rather than read.
/// 2. **Its directory index.** A portal built as many pages rather than one
///    bundle publishes `<section>/index.html`, and `rsync --recursive` lands it
///    under exactly that key. Reaching for it *before* the entrypoint is what
///    keeps such a build from answering every page with the wrong document,
///    which is worse than the 404 it would replace.
/// 3. **The entrypoint.** A single-bundle portal routes on the client, so a
///    path it published nothing for is a route rather than a miss, and the
///    entrypoint is what renders it.
///
/// The empty path is the bare mount and resolves to the entrypoint alone.
fn bundle_candidates(asset: &str) -> Vec<String> {
    let trimmed = asset.trim_end_matches('/');
    if trimmed.is_empty() {
        return vec![INDEX.to_string()];
    }

    let mut candidates = Vec::with_capacity(3);
    if !asset.ends_with('/') {
        candidates.push(asset.to_string());
    }
    candidates.push(format!("{trimmed}/{INDEX}"));
    candidates.push(INDEX.to_string());
    candidates
}

/// The three outcomes of a bundle read, kept distinct so a missing object can
/// fall back to `index.html` while a backend failure surfaces as `502` — never
/// as "nothing published".
enum Fetched {
    Found(cloud::StoredObject),
    Missing,
    Failed,
}

async fn fetch(storage: &Arc<dyn cloud::StorageService>, key: &str) -> Fetched {
    match storage.get(key).await {
        Ok(object) => Fetched::Found(object),
        Err(cloud::StorageError::NotFound(_)) => Fetched::Missing,
        Err(error) => {
            tracing::error!(error = %error, bundle_key = %key, "portal bundle read failed");
            Fetched::Failed
        }
    }
}

/// Build the streamed response for one bundle object.
///
/// `served` is the bundle-relative path actually read, which decides both the
/// content type and the cache policy: `index.html` names the live build and is
/// `no-store`; every content-hashed asset is immutable for a year. An
/// entrypoint also gets [`portal_banner_html`] spliced into it, so a
/// participant has a way back to `/app/projects/{project_code}`; every other
/// object streams unmodified.
fn bundle_response(served: &str, mut object: cloud::StoredObject, project_code: &str) -> Response {
    if is_index(served) {
        object.bytes = open_with_banner(&object.bytes, &portal_banner_html(project_code));
    }
    let mut response = object.bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type_for(served)),
    );
    headers.insert(
        header::CACHE_CONTROL,
        if is_index(served) {
            NO_STORE
        } else {
            IMMUTABLE_CACHE
        },
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, PORTAL_CSP);
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// The small Neon Law banner spliced into a portal's entrypoint.
///
/// A participant embedded in a Project's own third-party bundle has no
/// Navigator chrome around it otherwise — nothing on the page can get them
/// back to the matter that mounted it. The markup is plain, self-contained
/// HTML with inline styles rather than a themed component: the bundle links
/// none of Navigator's stylesheet, so a class name here would resolve to
/// nothing. `PORTAL_CSP`'s `style-src 'self' 'unsafe-inline'` is what allows
/// the inline styles; no script runs.
///
/// `project_code` is already validated by [`store::projects::is_valid_code`]
/// before this is called — lowercase letters, digits, and single hyphens only
/// — but it is still escaped here rather than trusted, so this function's
/// safety does not depend on staying downstream of that gate forever.
fn portal_banner_html(project_code: &str) -> String {
    let brand = views::brand::FIRM_BRAND.site_name;
    let href = format!("/app/projects/{}", escape_attr(project_code));
    format!(
        "<div style=\"position:sticky;top:0;z-index:2147483647;display:flex;\
         align-items:center;justify-content:space-between;gap:1rem;\
         padding:0.5rem 1rem;background:#0f1a2b;color:#f5f7fa;\
         font:600 14px/1.4 -apple-system,BlinkMacSystemFont,'Segoe UI',\
         Helvetica,Arial,sans-serif;box-sizing:border-box;\">\
         <span>{brand}</span>\
         <a href=\"{href}\" style=\"color:#f5f7fa;\">&larr; Back to project</a>\
         </div>"
    )
}

/// Insert `banner` as the first child of the document body.
///
/// The same shape as `portal::dioxus_app::open_with_banner` for Navigator's
/// own SSR surface, kept as a separate copy rather than a shared helper: that
/// one operates on a `String` a Dioxus render already validated as UTF-8, and
/// this one operates on bytes a third-party build produced, which carries no
/// such guarantee.
///
/// A document with no `<body>` — or bytes that are not valid UTF-8 — is
/// returned untouched. Dropping the banner is better than corrupting or
/// failing the response over a build shape this cannot recognize.
fn open_with_banner(html: &[u8], banner: &str) -> Vec<u8> {
    let Ok(html) = std::str::from_utf8(html) else {
        return html.to_vec();
    };
    let Some(start) = html.find("<body") else {
        return html.as_bytes().to_vec();
    };
    let Some(end) = html[start..].find('>').map(|offset| start + offset + 1) else {
        return html.as_bytes().to_vec();
    };
    format!("{}{banner}{}", &html[..end], &html[end..]).into_bytes()
}

/// Whether the served path is an entrypoint, which is what makes it
/// `no-store`.
///
/// The final segment is compared, so a multi-page build's `guide/index.html`
/// counts alongside the root one. Both name the build's content-hashed assets
/// and neither is content-hashed itself, so caching either for a year would pin
/// a page at assets a later publish has aged out.
fn is_index(served: &str) -> bool {
    served.rsplit('/').next() == Some(INDEX)
}

/// A bundle-relative path is safe when it names something inside the mount:
/// no leading slash, no backslash, no control characters, and no `.`/`..`/empty
/// segment that could climb out of the `<code>/portal/` prefix. The empty path
/// (the bare mount) is safe — it resolves to the entrypoint.
///
/// One or more trailing slashes are trimmed before those rules apply, because a
/// trailing slash is what a portal's own navigation emits: a section link is
/// `${base}${slug}/`, so every in-app link below the mount arrives with one. A
/// trailing slash climbs out of nothing — what it names is a directory index or
/// a client-side route, and [`bundle_candidates`] resolves both. `a//b/` stays
/// refused: trimming the tail never reaches an interior empty segment.
fn asset_path_is_safe(asset: &str) -> bool {
    let asset = asset.trim_end_matches('/');
    asset.is_empty()
        || (!asset.starts_with('/')
            && !asset.contains('\\')
            && !asset.chars().any(char::is_control)
            && asset
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."))
}

/// The content type for a bundle-relative path, by extension. Derived here
/// rather than trusting the stored object's type so a portal serves correctly
/// regardless of the backend that wrote it — and so an ES module always
/// arrives as `text/javascript`, which the browser requires to execute it.
fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        asset_path_is_safe, bundle_candidates, content_type_for, is_index, open_with_banner,
        portal_banner_html, Refused, INDEX, PROJECT_PORTAL_ASSET, PROJECT_PORTAL_PATH,
        PROJECT_PORTAL_ROOT,
    };
    use cloud::workspace::PORTAL_MOUNT_SEGMENT;

    /// Every refusal reads differently in a log.
    ///
    /// The caller gets one indistinguishable 404 from all of them, deliberately
    /// — so the log line is the *only* place the closed gate is named, and two
    /// branches sharing a phrase would silently merge in an operator's search.
    /// Deploy run 32102608866 is why this is asserted: the portal fixture
    /// failed three times against a 404 that named no gate at all, and nothing
    /// in the pod logs could narrow it.
    #[test]
    fn every_refusal_names_a_distinct_closed_gate() {
        let all = [
            Refused::NoSession,
            Refused::MalformedCode,
            Refused::NoSuchProject,
            Refused::NotParticipating,
            Refused::UnsafeAssetPath,
            Refused::NothingPublished,
        ];

        for refused in all {
            let reason = refused.reason();
            assert!(
                !reason.is_empty(),
                "{refused:?} logs an empty reason, which names nothing"
            );
        }

        for (index, refused) in all.iter().enumerate() {
            for other in &all[index + 1..] {
                assert_ne!(
                    refused.reason(),
                    other.reason(),
                    "{refused:?} and {other:?} log the same reason, so a 404 cannot \
                     distinguish them"
                );
            }
        }
    }

    /// The collision this segment exists to prevent.
    ///
    /// `/app/projects/{code}` is Navigator's matter show page and
    /// `/app/projects/{code}/portal` is a Project's own surface. They differ in
    /// path *shape* — two segments after `/app/projects` versus three — so
    /// neither can match the other's request and no Axum registration order
    /// decides between them. Asserting the shapes is what makes that structural
    /// rather than remembered; `portal/tests/project_portal_route.rs` asserts
    /// both actually resolve on the assembled router.
    #[test]
    fn the_portal_mount_cannot_shadow_the_matter_show_page() {
        assert_eq!(PROJECT_PORTAL_PATH, "/app/projects/{project_code}/portal");
        assert_eq!(
            crate::dioxus_app::PROJECT_DETAIL_PATH,
            "/app/projects/{project_code}",
            "the matter show page keeps its own mount"
        );

        let portal_segments = PROJECT_PORTAL_PATH.split('/').count();
        let detail_segments = crate::dioxus_app::PROJECT_DETAIL_PATH.split('/').count();
        assert_ne!(
            portal_segments, detail_segments,
            "the two mounts must differ in shape, not merely in registration order"
        );
    }

    /// The mount segment is a literal, so nothing supplies a name for it.
    #[test]
    fn the_mount_segment_is_a_literal_rather_than_a_parameter() {
        let last = PROJECT_PORTAL_PATH
            .rsplit('/')
            .next()
            .expect("the path has a final segment");
        assert_eq!(last, PORTAL_MOUNT_SEGMENT);
        assert!(
            !last.contains('{'),
            "an application name would make this a path parameter again"
        );
    }

    /// The three mounts share one prefix, so a bundle asset resolves under the
    /// same code the entrypoint does.
    #[test]
    fn the_asset_and_root_mounts_extend_the_bare_mount() {
        assert_eq!(PROJECT_PORTAL_ROOT, format!("{PROJECT_PORTAL_PATH}/"));
        assert_eq!(
            PROJECT_PORTAL_ASSET,
            format!("{PROJECT_PORTAL_PATH}/{{*asset}}")
        );
    }

    /// A traversal cannot climb out of the `<code>/portal/` prefix.
    #[test]
    fn a_traversal_path_is_refused() {
        assert!(asset_path_is_safe(""));
        assert!(asset_path_is_safe("index.html"));
        assert!(asset_path_is_safe("assets/index-abc123.js"));
        assert!(!asset_path_is_safe("../secret"));
        assert!(!asset_path_is_safe("assets/../../etc/passwd"));
        assert!(!asset_path_is_safe("/etc/passwd"));
        assert!(!asset_path_is_safe("a//b"));
        assert!(!asset_path_is_safe("a\\b"));
    }

    /// A trailing slash is a link a portal writes, not a traversal.
    ///
    /// Every section link a portal renders is `${base}${slug}/`, so this is the
    /// ordinary shape rather than an edge case — and reading it as an empty
    /// final segment is what made every section answer 404. Trimming the tail
    /// must not reach an interior empty segment, which is the `a//b/` case.
    #[test]
    fn a_trailing_slash_is_safe_but_an_interior_empty_segment_is_not() {
        assert!(asset_path_is_safe("/"));
        assert!(asset_path_is_safe("engagement/"));
        assert!(asset_path_is_safe("a/b/"));
        assert!(asset_path_is_safe("matters/open/filings/"));
        assert!(!asset_path_is_safe("a//b/"));
        assert!(!asset_path_is_safe("../secret/"));
        assert!(!asset_path_is_safe("assets/./app.js/"));
    }

    /// Every `index.html` is `no-store`, wherever it sits.
    ///
    /// A multi-page build's `guide/index.html` is as much an entrypoint as the
    /// root one: it names hashed assets and is not itself hashed, so a year of
    /// immutable caching would pin it at assets a later publish aged out.
    #[test]
    fn any_index_html_is_an_entrypoint() {
        assert!(is_index("index.html"));
        assert!(is_index("guide/index.html"));
        assert!(!is_index("assets/index-abc.js"));
        assert!(!is_index("indexes.html"));
    }

    /// The resolution order a request walks.
    ///
    /// The directory index sits *before* the entrypoint, which is the whole
    /// claim: without it a multi-page build answers every page with the root
    /// document — a 200 carrying the wrong content, which is worse than the 404
    /// it replaced. The entrypoint stays last so a single-bundle portal's
    /// client-side route still renders.
    #[test]
    fn a_path_resolves_to_itself_then_its_directory_index_then_the_entrypoint() {
        assert_eq!(bundle_candidates(""), [INDEX]);
        assert_eq!(bundle_candidates("/"), [INDEX]);

        assert_eq!(
            bundle_candidates("engagement/"),
            ["engagement/index.html", INDEX],
            "a trailing slash names no object, so only the index is read for"
        );
        assert_eq!(
            bundle_candidates("assets/app-abc.js"),
            ["assets/app-abc.js", "assets/app-abc.js/index.html", INDEX],
        );

        let candidates = bundle_candidates("matters/open");
        assert_eq!(candidates.first().map(String::as_str), Some("matters/open"));
        assert_eq!(candidates.last().map(String::as_str), Some(INDEX));
        assert!(
            candidates.contains(&"matters/open/index.html".to_string()),
            "the slashless form must find the directory index too"
        );
    }

    /// The banner names the brand and links back to the caller's own matter,
    /// with the code escaped rather than trusted even though the caller
    /// already validated it.
    #[test]
    fn the_banner_links_back_to_the_matter() {
        let html = portal_banner_html("libra-formation");
        assert!(html.contains("Neon Law"), "{html}");
        assert!(
            html.contains("href=\"/app/projects/libra-formation\""),
            "{html}"
        );
    }

    /// The banner lands as the body's first child, whatever attributes the
    /// opening tag carries, and a document with no body — or invalid UTF-8 —
    /// is returned untouched rather than corrupted.
    #[test]
    fn the_banner_opens_the_body_and_leaves_a_bodyless_document_alone() {
        let banner = "<div id=\"b\">B</div>";

        assert_eq!(
            open_with_banner(
                b"<html><head></head><body><div id=\"root\"></div></body></html>",
                banner
            ),
            b"<html><head></head><body><div id=\"b\">B</div><div id=\"root\"></div></body></html>"
        );
        // The bundle's own body may carry attributes; the whole opening tag is
        // skipped rather than a literal `<body>` matched.
        assert_eq!(
            open_with_banner(b"<body class=\"x\" data-y><p>P</p></body>", banner),
            b"<body class=\"x\" data-y><div id=\"b\">B</div><p>P</p></body>"
        );
        assert_eq!(
            open_with_banner(b"<p>fragment</p>", banner),
            b"<p>fragment</p>"
        );
        assert_eq!(open_with_banner(b"<body", banner), b"<body");
        // Invalid UTF-8 is returned byte-for-byte rather than lossily
        // re-encoded, which would silently corrupt a binary response.
        let invalid = [0xff, 0xfe, 0x00];
        assert_eq!(open_with_banner(&invalid, banner), invalid);
    }

    /// An ES module must arrive as `text/javascript` or the browser refuses to
    /// execute it; the rest cover the common bundle types.
    #[test]
    fn content_types_are_derived_from_the_extension() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("assets/app-abc.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type_for("assets/app-abc.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type_for("logo.svg"), "image/svg+xml");
        assert_eq!(content_type_for("font.woff2"), "font/woff2");
        assert_eq!(content_type_for("noextension"), "application/octet-stream");
    }
}
