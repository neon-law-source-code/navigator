//! Native POST for `/app/brands/{key}/edit`.
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

#[derive(Debug, Deserialize)]
pub struct BrandPresentationForm {
    pub typeface: String,
    pub primary_color: String,
    #[serde(default)]
    pub font_family: String,
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
    match apply_brand_presentation(
        &surreal,
        session.role,
        session.person_id,
        &key,
        &form.typeface,
        &form.primary_color,
        &form.font_family,
    )
    .await
    {
        Ok(_) => Redirect::to(&format!("/app/brands/{key}/edit")).into_response(),
        Err(crate::api::BrandPresentationError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(crate::api::BrandPresentationError::Forbidden) => StatusCode::FORBIDDEN.into_response(),
        Err(crate::api::BrandPresentationError::UnknownChoice(message)) => {
            let mut query = String::new();
            crate::admin::push_query(&mut query, "error", &message);
            Redirect::to(&format!("/app/brands/{key}/edit?{query}")).into_response()
        }
        Err(crate::api::BrandPresentationError::Internal(error)) => {
            tracing::error!(error = %error, "brand edit form failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
