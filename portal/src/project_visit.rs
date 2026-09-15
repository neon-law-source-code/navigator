//! Access telemetry for successful Project page renders.

use axum::extract::Request;
use axum::{
    extract::{MatchedPath, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};

use crate::session::SessionData;

/// Log one successful, authorized render of a Project page.
///
/// The middleware is attached only to GET page routes, but it also checks the
/// matched route and response status so redirects, refusals, API calls, and
/// static portal assets cannot become visits. `MatchedPath` is the route
/// template Axum stores in request extensions for middleware and extractors:
/// <https://docs.rs/axum/latest/axum/extract/struct.MatchedPath.html>.
pub(crate) async fn log_project_visit(
    State(surreal): State<store::surreal::SurrealDb>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::GET {
        return next.run(request).await;
    }
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned());
    let session = request.extensions().get::<SessionData>().cloned();
    let project_code = route
        .as_deref()
        .filter(|route| is_project_page_route(route))
        .and_then(|_| project_code_from_path(request.uri().path()))
        .map(str::to_owned);

    let response = next.run(request).await;
    if response.status() != StatusCode::OK {
        return response;
    }

    let (Some(route), Some(session), Some(project_code)) = (route, session, project_code) else {
        return response;
    };
    let Some(person_id) = session.person_id else {
        return response;
    };
    let Ok(Some(project)) = store::projects::find_by_code(&surreal, &project_code).await else {
        return response;
    };

    tracing::info!(
        target: "audit",
        audit = true,
        person_id = %person_id,
        project_id = %project.id,
        route = %route,
        role = session.role.as_str(),
        "project visited"
    );
    response
}

fn is_project_page_route(route: &str) -> bool {
    matches!(
        route,
        "/app/projects/{project_code}"
            | "/app/projects/{project_code}/portal/"
            | "/app/projects/{project_code}/edit"
            | "/app/projects/{project_code}/people/new"
            | "/app/projects/{project_code}/people/{role_id}/edit"
            | "/app/projects/{project_code}/notations/new"
            | "/app/projects/{project_code}/documents/{doc_id}"
            | "/app/projects/{project_code}/conversation"
            | "/app/projects/{project_code}/intake/{notation_id}"
            | "/app/projects/{project_code}/review/{doc_id}"
    )
}

fn project_code_from_path(path: &str) -> Option<&str> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    (segments.next() == Some("app") && segments.next() == Some("projects"))
        .then(|| segments.next())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{is_project_page_route, project_code_from_path};

    #[test]
    fn only_project_page_templates_are_visits() {
        assert!(is_project_page_route("/app/projects/{project_code}"));
        assert!(is_project_page_route(
            "/app/projects/{project_code}/documents/{doc_id}"
        ));
        assert!(!is_project_page_route("/app/projects"));
        assert!(!is_project_page_route("/app/api/projects/{id}"));
        assert!(!is_project_page_route(
            "/app/projects/{project_code}/documents/{doc_id}/download"
        ));
    }

    #[test]
    fn project_code_is_read_only_from_the_resolved_path() {
        assert_eq!(
            project_code_from_path("/app/projects/sample-litigation/documents/abc"),
            Some("sample-litigation")
        );
        assert_eq!(project_code_from_path("/app/projects"), None);
        assert_eq!(project_code_from_path("/app/api/projects/abc"), None);
    }
}
