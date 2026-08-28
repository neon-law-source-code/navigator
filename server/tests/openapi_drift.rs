#![allow(clippy::doc_markdown)]
//! Drift guard: the hand-curated OpenAPI document in
//! [`portal::openapi::document`] must describe exactly the `/app/api/*`
//! operations that [`portal::api::routes`] registers — matched at
//! `(HTTP method, path)` granularity, not path-only. Without this test
//! the doc silently rots whenever a new route or method lands.
//!
//! The path-only predecessor could not see method drift: a `PUT` added
//! to an already-listed path, or an undocumented alias sharing no path
//! key, slipped straight through. Comparing the exact `(method, path)`
//! set closes that gap. Crucially, `api::documented_api_operations()` is
//! *derived from the same table* `api::routes()` builds the router from,
//! so it cannot omit a registered route — an entirely new undocumented
//! path therefore fails this comparison against the document, not just a
//! new method on an existing path. The complementary
//! `web/tests/routes.rs::api_router_operations_match_openapi_document`
//! probes the *live* router as a second, runtime check.
//!
//! `/app/api/openapi.json` and `/app/api` are deliberately excluded — those are
//! documentation surfaces (the spec itself and the Swagger UI shell)
//! mounted outside the API gate by `api::doc_routes`, not part of the
//! public API surface the document describes.

use std::collections::BTreeSet;

#[test]
fn openapi_operations_match_registered_api_routes() {
    let registered: BTreeSet<(String, String)> = portal::api::documented_api_operations()
        .iter()
        .map(|(method, path)| ((*method).to_string(), (*path).to_string()))
        .collect();
    let documented: BTreeSet<(String, String)> = portal::openapi::documented_operations()
        .into_iter()
        .collect();
    assert_eq!(
        registered,
        documented,
        "OpenAPI document drift: the (method, path) operations registered in `api::routes()` \
         (and listed in `api::documented_api_operations`) must match the operations declared in \
         `openapi::document()[\"paths\"]`. \
         Only in routes = {:?}; only in doc = {:?}",
        registered.difference(&documented).collect::<Vec<_>>(),
        documented.difference(&registered).collect::<Vec<_>>(),
    );
}

/// The `kind` enum documented on `POST /app/api/projects/{id}/documents` must be
/// exactly the vocabulary `store::documents::ingest_bytes` accepts.
///
/// The document lists the values by hand, which is the readable form for a
/// generated client but the drifting one: widen or narrow
/// `rules::kind::Kind::valid_for(Lane::Asset)` and the published constraint
/// silently starts describing a rule that no longer exists. A caller reading
/// the document would then be told a value is accepted that the door refuses
/// with a 400 — worse than the undescribed bare string it replaced, because it
/// looks authoritative.
#[test]
fn the_documented_document_kinds_are_the_kinds_ingest_accepts() {
    let accepted: Vec<String> = rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
        .map(|k| k.as_str().to_string())
        .collect();

    let doc = portal::openapi::document();
    let documented: Vec<String> = doc["paths"]["/app/api/projects/{id}/documents"]["post"]
        ["requestBody"]["content"]["application/json"]["schema"]["properties"]["kind"]["enum"]
        .as_array()
        .expect("`kind` declares an enum of accepted values")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("each accepted kind is a string")
                .to_string()
        })
        .collect();

    assert_eq!(
        documented, accepted,
        "OpenAPI drift: the `kind` values documented on the document-upload operation must be          exactly the asset-lane kinds `ingest_bytes` accepts, in the same order"
    );
}
