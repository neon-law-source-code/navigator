//! Lawyer cron-schedule reference page as a Dioxus component (#956 Phase 4).
//!
//! The successor to the `views::pages::admin::schedules`. Lists the
//! Kubernetes `CronJob`s that drive Neon Law Navigator's scheduled work with
//! their cron expression, a human cadence, what each does, and a "Run now"
//! trigger.
//!
//! This is a *declared* reference — it documents the schedules that ship in
//! `examples/deploy/k8s/exports/`, not a live cluster read (`web` has no
//! Kubernetes API access). Keep [`CRON_JOBS`] in sync when a `CronJob` is
//! added; see `docs/cronjobs.md`.
//!
//! `POST /app/admin/schedules/{job}/run` stays on the `portal::cron_schedules`
//! handler and already follows post/redirect/get: it redirects back here with
//! `?notice=queued:{slug}` (or `not_queued`), which renders as a toast.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState, Toast, ToastTone};
use crate::people::ViewerRole;

/// The page's own path, and the base the "Run now" forms post under.
pub const SCHEDULES_PATH: &str = "/app/admin/schedules";

/// One scheduled job, as the page needs it.
pub struct CronJobEntry {
    /// Display name. Must match the name `portal::cron_schedules` knows for
    /// this `slug`, because that is what the redirect notice names back.
    pub name: &'static str,
    /// Cron expression (UTC), exactly as deployed.
    pub schedule: &'static str,
    /// Human cadence, in the workspace's Pacific convention.
    pub cadence: &'static str,
    /// What the job does.
    pub description: &'static str,
    /// The job's slug — both the `POST` route segment and the key the
    /// `?notice=` flash names.
    pub slug: &'static str,
}

impl CronJobEntry {
    /// The `POST` route that queues this job on demand.
    #[must_use]
    pub fn manual_run(&self) -> String {
        format!("{SCHEDULES_PATH}/{}/run", self.slug)
    }
}

/// The `CronJob`s Neon Law Navigator ships. Mirrors the manifests under
/// `examples/deploy/k8s/exports/`. Add a row when a `CronJob` is added.
pub const CRON_JOBS: &[CronJobEntry] = &[
    CronJobEntry {
        name: "Archives nightly export",
        schedule: "0 10 * * *",
        cadence: "Daily · 02:00 PST",
        description: "Snapshots every database table to Parquet on GCS, summarizes GCP cost, \
                      and emails the diagnostic report.",
        slug: "archives",
    },
    CronJobEntry {
        name: "Billing canary",
        schedule: "0 14 * * 0",
        cadence: "Weekly · Sun 06:00 PST",
        description: "Find-or-creates one stable canary contact in Xero, then emails a \
                      confirmation — a two-step Restate workflow that proves the billing \
                      integration still agrees with Xero's API.",
        slug: "billing-canary",
    },
    CronJobEntry {
        name: "Billing digest",
        schedule: "0 13 * * *",
        cadence: "Daily · 05:00 PST",
        description: "Posts firm ops a trailing-30-day GCP cost report covering every configured \
                      billing account and every project on each — a cross-account roll-up, then \
                      per account a table by project and one by service, each against the prior \
                      30 days. A two-step Restate workflow; a no-op where no billing export is \
                      configured.",
        slug: "billing-digest",
    },
    CronJobEntry {
        name: "Durable-execution heartbeat",
        schedule: "0 */6 * * *",
        cadence: "Every 6h · 00/06/12/18 UTC",
        description: "Liveness canary for the durable-execution engine itself — a two-step \
                      Restate workflow (beat → notify) that depends on nothing (no database, \
                      no object storage, no third-party API), so a green run can only mean the \
                      engine accepted an invocation and ran it to completion. Emails firm ops \
                      every six hours with the Restate Cloud + GCP console links to check when \
                      a beat is missing.",
        slug: "heartbeat",
    },
    CronJobEntry {
        name: "Invoice reconciliation",
        schedule: "0 11 * * *",
        cadence: "Daily · 03:00 PST",
        description: "Refreshes every unsettled Xero invoice in the local mirror so the \
                      portal's invoice list reflects current payment status.",
        slug: "reconcile-invoices",
    },
];

/// The flash a manual run redirects back with, already resolved to a message.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ScheduleNotice {
    /// The rendered sentence, e.g. `Archives nightly export queued.`
    pub message: String,
    /// Whether the job was queued — selects the toast tone.
    pub queued: bool,
}

/// Resolve a `?notice=` value to its flash.
///
/// The value is `{outcome}:{slug}`, written by `portal::cron_schedules`'s
/// redirect. An unknown outcome or slug resolves to `None` rather than
/// rendering attacker-supplied text, so the query cannot inject a message.
#[must_use]
pub fn resolve_notice(value: Option<&str>) -> Option<ScheduleNotice> {
    let (outcome, slug) = value?.split_once(':')?;
    let job = CRON_JOBS.iter().find(|job| job.slug == slug)?;
    match outcome {
        "queued" => Some(ScheduleNotice {
            message: format!("{} queued.", job.name),
            queued: true,
        }),
        "not_queued" => Some(ScheduleNotice {
            message: format!("{} could not be queued.", job.name),
            queued: false,
        }),
        _ => None,
    }
}

/// The rendered schedules page: the session CSRF token the "Run now" forms
/// carry, the viewer's tier, and the resolved `?notice=` flash.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SchedulesView {
    pub csrf_token: String,
    pub role: ViewerRole,
    #[serde(default)]
    pub notice: Option<ScheduleNotice>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The `?notice=` flash a manual run redirects back with.
#[derive(Deserialize, Default)]
pub struct SchedulePageQuery {
    #[serde(default)]
    pub notice: Option<String>,
}

/// Load the schedules page: refuse non-lawyer, then read the CSRF token and the
/// `?notice=` flash. The job list itself is a compile-time constant.
#[server]
pub async fn get_schedules() -> Result<SchedulesView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;

    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<SchedulePageQuery>,
        _,
    >()
    .await?;

    Ok(SchedulesView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        csrf_token,
        role,
        notice: resolve_notice(query.notice.as_deref()),
    })
}

/// The lawyer cron-schedules page. Server-side rendered: each "Run now" is a
/// native `POST` carrying the CSRF token, so it works without JavaScript.
#[component]
pub fn LawyerSchedules() -> Element {
    let resource = use_server_future(get_schedules)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "schedules", p { "Failed to load schedules." } }
            }
        }
        None => {
            return rsx! {
                main { id: "schedules", p { "Loading…" } }
            }
        }
    };

    schedules_body(&view)
}

/// The loaded page. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn schedules_body(view: &SchedulesView) -> Element {
    let view = view.clone();
    let columns = vec![
        Column::fixed("job", "Job"),
        Column::fixed("schedule", "Schedule (UTC)"),
        Column::fixed("cadence", "Cadence"),
        Column::fixed("description", "What it does"),
        Column::fixed("run", "Run now"),
    ];

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Cron schedules" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if view.role.is_lawyer_tier() {
                a { class: "nav-link", href: "/lawyer", "Lawyer" }
            }
            if view.role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "schedules", class: "nav-theme",
            if let Some(notice) = view.notice.as_ref() {
                Toast {
                    message: notice.message.clone(),
                    tone: if notice.queued { ToastTone::Success } else { ToastTone::Danger },
                }
            }
            header { class: "page-header",
                h1 { "Cron schedules" }
                p { class: "nav-muted",
                    "Scheduled jobs that run on the cluster, shown with the cron expression "
                    "(UTC) and the Pacific cadence. Run any listed job on demand here."
                }
            }
            DataTable {
                columns,
                sort: SortState::default(),
                base_path: SCHEDULES_PATH.to_string(),
                for job in CRON_JOBS.iter() {
                    tr {
                        td { strong { "{job.name}" } }
                        td { code { "{job.schedule}" } }
                        td { "{job.cadence}" }
                        td { "{job.description}" }
                        td {
                            form { method: "post", action: job.manual_run(),
                                input {
                                    r#type: "hidden",
                                    name: "_csrf",
                                    value: "{view.csrf_token}",
                                }
                                button {
                                    r#type: "submit",
                                    class: "nav-btn nav-btn--primary",
                                    "Run now"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> SchedulesView {
        SchedulesView {
            firm_name: "Neon Law".to_string(),
            csrf_token: "tok-9".into(),
            role: ViewerRole::Lawyer,
            notice: None,
        }
    }

    #[test]
    fn lists_the_archives_export_with_schedule_and_trigger() {
        let html = dioxus_ssr::render_element(schedules_body(&view()));
        assert!(html.contains("Archives nightly export"), "{html}");
        assert!(html.contains("0 10 * * *"), "{html}");
        assert!(html.contains("02:00 PST"), "{html}");
        // The manual trigger renders as a CSRF-protected POST form.
        assert!(
            html.contains("action=\"/app/admin/schedules/archives/run\""),
            "{html}"
        );
        assert!(html.contains("name=\"_csrf\""), "{html}");
        assert!(html.contains("value=\"tok-9\""), "{html}");
        assert!(html.contains("Run now"), "{html}");
    }

    #[test]
    fn lists_the_billing_digest_daily_workflow() {
        let html = dioxus_ssr::render_element(schedules_body(&view()));
        assert!(html.contains("Billing digest"), "{html}");
        assert!(html.contains("0 13 * * *"), "{html}");
        assert!(html.contains("05:00 PST"), "{html}");
        assert!(html.contains("trailing-30-day GCP cost"), "{html}");
    }

    #[test]
    fn lists_the_durable_execution_heartbeat() {
        let html = dioxus_ssr::render_element(schedules_body(&view()));
        assert!(html.contains("Durable-execution heartbeat"), "{html}");
        assert!(html.contains("0 */6 * * *"), "{html}");
        assert!(html.contains("Every 6h"), "{html}");
        assert!(html.contains("Liveness canary"), "{html}");
    }

    #[test]
    fn lists_invoice_reconciliation_with_a_manual_trigger() {
        let html = dioxus_ssr::render_element(schedules_body(&view()));
        assert!(html.contains("Invoice reconciliation"), "{html}");
        assert!(
            html.contains("action=\"/app/admin/schedules/reconcile-invoices/run\""),
            "{html}"
        );
    }

    #[test]
    fn floats_a_queued_notice() {
        let html = dioxus_ssr::render_element(schedules_body(&SchedulesView {
            notice: resolve_notice(Some("queued:archives")),
            ..view()
        }));
        assert!(html.contains("Archives nightly export queued."), "{html}");
        assert!(html.contains("nav-toast--success"), "{html}");
    }

    #[test]
    fn floats_a_failure_notice() {
        let html = dioxus_ssr::render_element(schedules_body(&SchedulesView {
            notice: resolve_notice(Some("not_queued:heartbeat")),
            ..view()
        }));
        assert!(
            html.contains("Durable-execution heartbeat could not be queued."),
            "{html}"
        );
        assert!(html.contains("nav-toast--danger"), "{html}");
    }

    #[test]
    fn an_unknown_notice_resolves_to_nothing() {
        // The query is attacker-controlled, so only a known outcome+slug pair
        // renders — never the raw value.
        assert!(resolve_notice(None).is_none());
        assert!(resolve_notice(Some("queued:not-a-job")).is_none());
        assert!(resolve_notice(Some("exploded:archives")).is_none());
        assert!(resolve_notice(Some("no-colon")).is_none());
        assert!(resolve_notice(Some("<script>:archives")).is_none());
    }

    #[test]
    fn every_job_carries_a_schedule_cadence_and_slug() {
        for job in CRON_JOBS {
            assert!(!job.schedule.is_empty(), "{} missing schedule", job.name);
            assert!(!job.cadence.is_empty(), "{} missing cadence", job.name);
            assert!(!job.slug.is_empty(), "{} missing slug", job.name);
            assert_eq!(
                job.manual_run(),
                format!("/app/admin/schedules/{}/run", job.slug)
            );
        }
    }
}
