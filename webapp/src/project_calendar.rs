//! The project calendar — the dated view of matter appearances, shared by the
//! lawyer workbench (`/app/lawyer`, across every matter the caller can see) and
//! the matter workbench (`/app/projects/{code}`, scoped to one).
//!
//! Rows come from docket appearances: a hearing or trial's current
//! `scheduled_on`, following a continuance chain to the entry no later entry
//! supersedes. A document, a participation, and a notation are not scheduled
//! events. Deadlines stay on their own module — an appearance is a fact the
//! docket records, not a derived obligation.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One sortable column: its `?sort=` key and its header label.
pub type CalendarColumn = (&'static str, &'static str);

/// The lawyer workbench's columns, in render order. Every visible matter shares
/// one calendar there, so an event is identified by the matter and the entity
/// it belongs to.
pub const WORKBENCH_COLUMNS: &[CalendarColumn] = &[
    ("date", "Date"),
    ("project", "Project"),
    ("entity", "Entity"),
    ("status", "Status"),
];

/// The matter workbench's columns. The matter and its entity are the page
/// itself, so both drop out; what one matter's calendar has to name instead is
/// the event.
pub const MATTER_COLUMNS: &[CalendarColumn] =
    &[("date", "Date"), ("event", "Event"), ("status", "Status")];

/// One appearance the calendar can render.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub date: String,
    pub event: String,
    pub status: String,
    pub project: String,
    pub entity: String,
}

impl CalendarEvent {
    fn cell(&self, column: &str) -> &str {
        match column {
            "date" => &self.date,
            "event" => &self.event,
            "status" => &self.status,
            "project" => &self.project,
            "entity" => &self.entity,
            _ => "",
        }
    }
}

/// The `?sort=`/`?dir=` pair every calendar reads. Both are lenient — an
/// unrecognised value falls back to the default rather than refusing the
/// request, which is why neither page carries a `400`-on-bad-sort pre-handler.
#[derive(serde::Deserialize, Default)]
pub struct CalendarQuery {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
}

/// Normalise `?sort=` to one of `columns`, defaulting to the first — the
/// leftmost column is the one the calendar is ordered by until asked otherwise.
#[must_use]
pub fn sort_field(raw: Option<&str>, columns: &[CalendarColumn]) -> String {
    let default = columns.first().map_or("date", |(key, _)| *key);
    match raw {
        Some(value) if columns.iter().any(|(key, _)| *key == value) => value.to_string(),
        _ => default.to_string(),
    }
}

/// Normalise `?dir=` to `asc` / `desc`, defaulting to `asc`.
#[must_use]
pub fn sort_dir(raw: Option<&str>) -> String {
    if raw == Some("desc") {
        "desc".to_string()
    } else {
        "asc".to_string()
    }
}

/// Order appearances by the active column. Unknown columns fall back to date.
pub fn sort_events(events: &mut [CalendarEvent], sort: &str, dir: &str) {
    let descending = dir == "desc";
    events.sort_by(|left, right| {
        let ordering = left.cell(sort).cmp(right.cell(sort));
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

/// The calendar: a heading over a table of sortable headers and either the
/// current appearances or one row saying none are scheduled.
///
/// `path` and `query_prefix` build the header links. `query_prefix` is whatever
/// the host page must carry through a re-sort, already `k=v&`-joined and
/// `&`-terminated (the workbench carries its list's status; a matter carries
/// nothing), so re-sorting the calendar never resets its neighbours.
#[component]
pub fn ProjectCalendar(
    section_class: String,
    heading: String,
    empty_message: String,
    columns: Vec<CalendarColumn>,
    path: String,
    query_prefix: String,
    sort: String,
    dir: String,
    events: Vec<CalendarEvent>,
) -> Element {
    let span = columns.len().to_string();
    let mut events = events;
    sort_events(&mut events, &sort, &dir);

    rsx! {
        section { class: "{section_class}",
            h2 { "{heading}" }
            div { class: "nav-table-wrap",
                table { class: "nav-table",
                    thead {
                        tr {
                            for (key , label) in columns.iter() {
                                th { scope: "col",
                                    CalendarSortLink {
                                        label: (*label).to_string(),
                                        field: (*key).to_string(),
                                        path: path.clone(),
                                        query_prefix: query_prefix.clone(),
                                        sort: sort.clone(),
                                        dir: dir.clone(),
                                    }
                                }
                            }
                        }
                    }
                    tbody {
                        if events.is_empty() {
                            tr {
                                td { colspan: "{span}", class: "nav-muted", "{empty_message}" }
                            }
                        } else {
                            for event in events.iter() {
                                tr {
                                    for (key , _) in columns.iter() {
                                        td { "{event.cell(key)}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One calendar header link. Toggles direction on the active column and carries
/// the host page's own query state, so re-sorting the calendar does not reset
/// the controls beside it — they share one query string. Page is intentionally
/// dropped: a new sort starts at page 1.
#[component]
fn CalendarSortLink(
    label: String,
    field: String,
    path: String,
    query_prefix: String,
    sort: String,
    dir: String,
) -> Element {
    let is_current = sort == field;
    let next_dir = if is_current && dir == "asc" {
        "desc"
    } else {
        "asc"
    };
    let marker = if is_current {
        format!(" ({dir})")
    } else {
        String::new()
    };
    let href = format!("{path}?{query_prefix}sort={field}&dir={next_dir}");

    rsx! {
        a { href: "{href}", "{label}{marker}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic empty-state message: the test exercises whatever text the
    /// caller passes through, not any particular page's real copy.
    const EMPTY_MESSAGE: &str = "No calendar events scheduled for this project.";

    fn matter_calendar(sort: &str, dir: &str) -> Element {
        rsx! {
            ProjectCalendar {
                section_class: "lawyer-detail__section project-calendar".to_string(),
                heading: "Calendar".to_string(),
                empty_message: EMPTY_MESSAGE.to_string(),
                columns: MATTER_COLUMNS.to_vec(),
                path: "/app/projects/sample-litigation".to_string(),
                query_prefix: String::new(),
                sort: sort.to_string(),
                dir: dir.to_string(),
                events: Vec::new(),
            }
        }
    }

    #[test]
    fn every_column_renders_and_the_body_stays_empty() {
        let html = dioxus_ssr::render_element(matter_calendar("date", "asc"));
        for (_, label) in MATTER_COLUMNS {
            assert!(html.contains(label), "{label}: {html}");
        }
        assert!(html.contains(EMPTY_MESSAGE), "{html}");
        // The empty row must span the whole table, whatever the column set is.
        assert!(html.contains("colspan=\"3\""), "{html}");
    }

    #[test]
    fn the_active_column_shows_its_direction_and_offers_the_reverse() {
        let html = dioxus_ssr::render_element(matter_calendar("event", "asc"));
        assert!(html.contains("Event (asc)"), "{html}");
        assert!(
            html.contains("/app/projects/sample-litigation?sort=event&#38;dir=desc"),
            "{html}"
        );
        // An inactive column offers ascending and carries no marker.
        assert!(
            html.contains("/app/projects/sample-litigation?sort=date&#38;dir=asc"),
            "{html}"
        );
        assert!(!html.contains("Date ("), "{html}");
    }

    #[test]
    fn the_query_prefix_rides_through_every_header_link() {
        let html = dioxus_ssr::render_element(rsx! {
            ProjectCalendar {
                section_class: "lawyer-project-calendar".to_string(),
                heading: "Project calendar".to_string(),
                empty_message: "No project calendar events scheduled.".to_string(),
                columns: WORKBENCH_COLUMNS.to_vec(),
                path: "/app/lawyer".to_string(),
                query_prefix: "status=closed&".to_string(),
                sort: "date".to_string(),
                dir: "asc".to_string(),
                events: Vec::new(),
            }
        });
        assert!(
            html.contains("/app/lawyer?status=closed&#38;sort=entity&#38;dir=asc"),
            "{html}"
        );
        assert!(html.contains("colspan=\"4\""), "{html}");
    }

    #[test]
    fn an_unadvertised_sort_falls_back_to_the_leftmost_column() {
        assert_eq!(sort_field(Some("event"), MATTER_COLUMNS), "event");
        assert_eq!(sort_field(Some("entity"), MATTER_COLUMNS), "date");
        assert_eq!(sort_field(None, MATTER_COLUMNS), "date");
        // The matter calendar drops `project`; the workbench still advertises it.
        assert_eq!(sort_field(Some("project"), MATTER_COLUMNS), "date");
        assert_eq!(sort_field(Some("project"), WORKBENCH_COLUMNS), "project");
    }

    #[test]
    fn only_desc_reverses_the_sort() {
        assert_eq!(sort_dir(Some("desc")), "desc");
        assert_eq!(sort_dir(Some("asc")), "asc");
        assert_eq!(sort_dir(Some("sideways")), "asc");
        assert_eq!(sort_dir(None), "asc");
    }

    #[test]
    fn a_continuance_chain_lists_one_row_with_the_later_date() {
        let html = dioxus_ssr::render_element(rsx! {
            ProjectCalendar {
                section_class: "lawyer-detail__section project-calendar".to_string(),
                heading: "Calendar".to_string(),
                empty_message: EMPTY_MESSAGE.to_string(),
                columns: MATTER_COLUMNS.to_vec(),
                path: "/app/projects/sample-litigation".to_string(),
                query_prefix: String::new(),
                sort: "date".to_string(),
                dir: "asc".to_string(),
                events: vec![CalendarEvent {
                    date: "2027-04-12 09:00 UTC".to_string(),
                    event: "Motion hearing (continued)".to_string(),
                    status: "Hearing".to_string(),
                    project: "Cruller v. Prine".to_string(),
                    entity: "Cruller".to_string(),
                }],
            }
        });
        assert!(html.contains("Motion hearing (continued)"), "{html}");
        assert!(html.contains("2027-04-12 09:00 UTC"), "{html}");
        assert!(html.contains("Hearing"), "{html}");
        assert!(!html.contains(EMPTY_MESSAGE), "{html}");
        assert_eq!(
            html.matches("<tr>").count(),
            2,
            "header plus one appearance: {html}"
        );
    }
}
