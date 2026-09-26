//! Native POST for `/app/admin/brands/{key}/edit`.
//!
//! The JSON command is `PATCH /app/api/brands/{key}`. This form twin
//! redirects back to the edit page so a native `<form>` stays post/redirect/get.

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use serde::Deserialize;

use crate::api::apply_brand_presentation;
use crate::session::SessionData;

/// `typeface` carries the chosen uploaded font family name directly (ENG-659:
/// the Admin edit page's select lists only this Firm's own uploads, never
/// the compiled catalog), or is absent when the Firm has none to choose from
/// yet — the page renders no control at all in that case. There is no
/// separate `font_family` field any more; choosing a family sets both of the
/// brand's font fields together, translated below.
#[derive(Debug, Deserialize)]
pub struct BrandPresentationForm {
    #[serde(default)]
    pub typeface: Option<String>,
    pub primary_color: String,
}

pub async fn post_brand_edit(
    State(surreal): State<store::surreal::SurrealDb>,
    Path(key): Path<String>,
    session: Option<Extension<SessionData>>,
    Form(form): Form<BrandPresentationForm>,
) -> Response {
    let Some(Extension(session)) = session else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // An empty or absent selection means "leave the font unchanged" — a
    // Firm with no uploaded font yet has nothing valid to submit, and
    // `apply_brand_presentation` treats `None` that way for both fields
    // rather than erroring on a blank `typeface`.
    let chosen_family = form.typeface.as_deref().map(str::trim).filter(|value| !value.is_empty());
    let typeface = chosen_family.map(|_| "uploaded");
    match apply_brand_presentation(
        &surreal,
        session.role,
        session.person_id,
        &key,
        typeface,
        &form.primary_color,
        chosen_family,
    )
    .await
    {
        Ok(_) => Redirect::to(&format!("/app/admin/brands/{key}/edit")).into_response(),
        Err(crate::api::BrandPresentationError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(crate::api::BrandPresentationError::UnknownChoice(message)) => {
            let mut query = String::new();
            crate::admin::push_query(&mut query, "error", &message);
            Redirect::to(&format!("/app/admin/brands/{key}/edit?{query}")).into_response()
        }
        Err(crate::api::BrandPresentationError::Internal(error)) => {
            tracing::error!(error = %error, "brand edit form failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
