//! Admin playbook writes — `POST /app/admin/playbooks` and
//! `POST /app/admin/playbooks/{id}`.
//!
//! A **playbook** is a client Entity's set of negotiating positions, the
//! yardstick the inbound-contract review measures a third-party contract
//! against (see [`crate::contract_review_walk`]). This surface lets an
//! attorney create a playbook for a Company and edit its positions. The three
//! `GET` renders live in `webapp::playbooks`; both write doors below follow
//! post/redirect/get back to them.
//!
//! Positions are entered as one textarea, one position per line,
//! pipe-delimited: `topic | preferred | fallback | walk-away | severity`.
//! [`parse_positions`] and [`store::playbooks::positions_to_text`] are the
//! round-trip between that text and [`store::playbooks::Position`].
//!
//! Every refusal carries the rejected positions text back in the query. A
//! position set is dozens of hand-authored lines, so bouncing an attorney to a
//! form reloaded from the stored row would silently discard the whole block
//! over one typo'd severity.

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use store::playbooks::{
    self, PlaybookError, Position, SEVERITY_HIGH, SEVERITY_LOW, SEVERITY_MEDIUM,
};

use crate::admin::push_query;
use crate::dioxus_app::{LAWYER_PLAYBOOKS_PATH, LAWYER_PLAYBOOK_NEW_PATH};

#[derive(Deserialize)]
pub struct CreateInput {
    entity_id: Uuid,
    name: String,
    positions: String,
}

/// `POST /app/admin/playbooks` — create a playbook for a Company.
pub async fn create(
    State(surreal): State<store::surreal::SurrealDb>,
    Form(input): Form<CreateInput>,
) -> Response {
    if input.name.trim().is_empty() {
        return back_to_new(&input, "A playbook name is required.");
    }
    let positions = match parse_positions(&input.positions) {
        Ok(p) if p.is_empty() => return back_to_new(&input, "Enter at least one position."),
        Ok(p) => p,
        Err(e) => return back_to_new(&input, &e),
    };
    match playbooks::create(
        &surreal,
        &playbooks::NewPlaybook {
            entity_id: input.entity_id,
            name: input.name.trim(),
            positions: &positions,
        },
    )
    .await
    {
        Ok(_) => Redirect::to(LAWYER_PLAYBOOKS_PATH).into_response(),
        Err(PlaybookError::DuplicateName(_)) => back_to_new(
            &input,
            "That Company already has a playbook with that name.",
        ),
        Err(e) => {
            tracing::error!(error = %e, "admin: create playbook failed");
            back_to_new(&input, "Could not create the playbook.")
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateInput {
    positions: String,
}

/// `POST /app/admin/playbooks/{id}` — replace the position set.
pub async fn update(
    State(surreal): State<store::surreal::SurrealDb>,
    Path(id): Path<Uuid>,
    Form(input): Form<UpdateInput>,
) -> Response {
    // An unknown id is a missing resource, not a refusal to correct: there is
    // no form to bounce back to, so it stays a 404.
    if playbooks::by_id(&surreal, id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    }
    let positions = match parse_positions(&input.positions) {
        Ok(p) if p.is_empty() => {
            return back_to_edit(id, &input.positions, "Enter at least one position.")
        }
        Ok(p) => p,
        Err(e) => return back_to_edit(id, &input.positions, &e),
    };
    match playbooks::update_positions(&surreal, id, &positions).await {
        Ok(()) => Redirect::to(LAWYER_PLAYBOOKS_PATH).into_response(),
        Err(e) => {
            tracing::error!(error = %e, %id, "admin: update playbook positions failed");
            back_to_edit(id, &input.positions, "Could not save the positions.")
        }
    }
}

// --- post/redirect/get -----------------------------------------------------

/// Redirect a refused create back to the create form with `message` as the
/// `?error=` flash and the whole submission echoed, so the correction is one
/// edit rather than a full retype.
fn back_to_new(input: &CreateInput, message: &str) -> Response {
    let mut query = String::new();
    push_query(&mut query, "error", message);
    push_query(&mut query, "entity_id", &input.entity_id.to_string());
    push_query(&mut query, "name", &input.name);
    push_query(&mut query, "positions", &input.positions);
    redirect_with_query(LAWYER_PLAYBOOK_NEW_PATH, &query)
}

/// Redirect a refused update back to that playbook's edit form, carrying the
/// message and the rejected positions text.
fn back_to_edit(id: Uuid, positions: &str, message: &str) -> Response {
    let mut query = String::new();
    push_query(&mut query, "error", message);
    push_query(&mut query, "positions", positions);
    redirect_with_query(&format!("/app/admin/playbooks/{id}/edit"), &query)
}

fn redirect_with_query(path: &str, query: &str) -> Response {
    if query.is_empty() {
        Redirect::to(path).into_response()
    } else {
        Redirect::to(&format!("{path}?{query}")).into_response()
    }
}

// --- textarea -> positions -------------------------------------------------

/// Parse the textarea into a position set. One position per non-blank line,
/// five `|`-separated fields, the last a valid severity. Returns a
/// user-facing error string naming the offending line.
///
/// # Errors
///
/// A line without exactly five fields, an empty topic, or an unrecognised
/// severity.
pub fn parse_positions(text: &str) -> Result<Vec<Position>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if parts.len() != 5 {
            return Err(format!(
                "Line {}: expected 5 fields separated by '|' (topic | preferred | fallback | \
                 walk-away | severity), got {}.",
                i + 1,
                parts.len()
            ));
        }
        if parts[0].is_empty() {
            return Err(format!("Line {}: the topic is required.", i + 1));
        }
        let severity = parts[4].to_lowercase();
        if ![SEVERITY_LOW, SEVERITY_MEDIUM, SEVERITY_HIGH].contains(&severity.as_str()) {
            return Err(format!(
                "Line {}: severity must be low, medium, or high (got \"{}\").",
                i + 1,
                parts[4]
            ));
        }
        out.push(Position {
            topic: parts[0].to_string(),
            preferred: parts[1].to_string(),
            fallback: parts[2].to_string(),
            walkaway: parts[3].to_string(),
            severity,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::parse_positions;
    use store::playbooks::{positions_to_text, Position, SEVERITY_HIGH};

    #[test]
    fn parses_well_formed_lines_and_normalises_severity() {
        let text = "Liability | mutual cap | 2x fees | uncapped | HIGH\n\
                    Governing law | Nevada | Delaware | no nexus | medium";
        let positions = parse_positions(text).unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0].topic, "Liability");
        assert_eq!(positions[0].walkaway, "uncapped");
        assert_eq!(positions[0].severity, SEVERITY_HIGH);
        assert_eq!(positions[1].severity, "medium");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let text = "\nLiability | a | b | c | low\n\n";
        assert_eq!(parse_positions(text).unwrap().len(), 1);
    }

    #[test]
    fn wrong_field_count_is_rejected_with_line_number() {
        let err = parse_positions("Liability | a | b | high").unwrap_err();
        assert!(err.contains("Line 1"));
        assert!(err.contains("5 fields"));
    }

    #[test]
    fn unknown_severity_is_rejected() {
        let err = parse_positions("Liability | a | b | c | critical").unwrap_err();
        assert!(err.contains("severity must be"));
    }

    #[test]
    fn round_trips_through_text() {
        // The renderer lives in `store::playbooks` so the form that prefills the
        // textarea and this parser cannot drift apart; the round trip is what
        // holds them together.
        let positions = vec![Position {
            topic: "Term".into(),
            preferred: "1 year".into(),
            fallback: "2 years".into(),
            walkaway: "perpetual".into(),
            severity: "medium".into(),
        }];
        let text = positions_to_text(&positions);
        assert_eq!(parse_positions(&text).unwrap(), positions);
    }
}
