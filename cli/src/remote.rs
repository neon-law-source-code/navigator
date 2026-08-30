//! Authenticated HTTP client for the `navigator` CLI.
//!
//! Every command here is a thin wrapper over an **existing** `web` route,
//! sent with `Authorization: Bearer <token>` — no parallel JSON API. The
//! server resolves the bearer back into the caller's session and runs the
//! same handler the browser does, so `is_lawyer_tier`, the `lawyer_review`
//! gate, and `authored_by` provenance all hold unchanged.
//!
//! | command | route |
//! | --- | --- |
//! | `projects list` | `GET /app/projects.csv` |
//! | `project open`   | `GET /app/projects/:code` |
//! | `document upload` | `POST /app/api/projects/{id}/documents` |
//! | `notation create`  | `POST /app/projects/{project_code}/notations/new` |
//! | `notation status`  | `GET /app/lawyer/notations/:id/review?format=json` |

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use comfy_table::{presets::UTF8_FULL, Cell, ContentArrangement, Table};
use serde::Deserialize;
use uuid::Uuid;

use crate::credentials::{self, default_credentials_path, HostCredential};
use crate::login::resolve_base;
use crate::palette;

/// Resolve `(base_url, bearer_token)` for `host`, erroring clearly when
/// there's no login or the stored token has expired.
pub(crate) fn resolve(host: Option<&str>) -> Result<(String, String)> {
    let creds = credentials::load(&default_credentials_path())?;
    let base = resolve_base(host, &creds)?;
    let cred: &HostCredential = creds
        .get(&base)
        .ok_or_else(|| anyhow!("not logged in to {base} — run `navigator site login --host …`"))?;
    if cred.is_expired(now_secs()) {
        return Err(anyhow!(
            "the stored token for {base} has expired — run `navigator site login --host {base}`"
        ));
    }
    Ok((base, cred.token.clone()))
}

/// `navigator site import <model> <file> [--overwrite]` — submit one standard
/// seed YAML document to the logged-in deployment. The CLI deliberately reads
/// no `SurrealDB` environment: authentication, authorization, lookup, and the
/// typed write boundary all belong to the server.
pub async fn seed(host: Option<&str>, model: &str, file: &Path, overwrite: bool) -> ExitCode {
    run(async {
        let yaml = std::fs::read_to_string(file)
            .with_context(|| format!("read seed file {}", file.display()))?;
        let (base, token) = resolve(host)?;
        let response = reqwest::Client::new()
            .post(format!("{base}/app/api/seed"))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "model": model,
                "yaml": yaml,
                "overwrite": overwrite,
            }))
            .send()
            .await
            .context("POST /app/api/seed")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "seed {model} failed: {status}: {}",
                first_line(&body)
            ));
        }
        println!("{body}");
        Ok(())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct VisibleProject {
    id: Uuid,
    code: String,
}

/// `navigator site document upload --project <code> --file … --kind …`
/// — file a local document into a matter through the REST door
/// (`POST /app/api/projects/{id}/documents`). `--kind` is required and must
/// be an asset-lane classification; the CLI does not default it.
pub async fn document_upload(
    host: Option<&str>,
    project_code: &str,
    file: &Path,
    kind: &str,
    visibility: Option<&str>,
    description: Option<&str>,
    content_type: Option<&str>,
) -> ExitCode {
    run(async {
        let bytes = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
        let filename = file
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| anyhow!("file path has no filename"))?;
        let (base, token) = resolve(host)?;
        let client = reqwest::Client::new();
        let list = client
            .get(format!("{base}/app/api/projects"))
            .bearer_auth(&token)
            .send()
            .await
            .context("GET /app/api/projects")?;
        let list_status = list.status();
        let list_body = list.text().await.unwrap_or_default();
        if !list_status.is_success() {
            return Err(anyhow!(
                "list projects failed: {list_status}: {}",
                first_line(&list_body)
            ));
        }
        let projects: Vec<VisibleProject> =
            serde_json::from_str(&list_body).context("parse GET /app/api/projects")?;
        let project = projects
            .iter()
            .find(|p| p.code == project_code)
            .ok_or_else(|| anyhow!("no visible matter with code `{project_code}`"))?;
        let content_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut body = serde_json::json!({
            "filename": filename,
            "content_base64": content_base64,
            "content_type": content_type.unwrap_or("application/octet-stream"),
            "kind": kind,
            "visibility": visibility.unwrap_or("internal"),
        });
        if let Some(description) = description.map(str::trim).filter(|s| !s.is_empty()) {
            body["description"] = serde_json::Value::String(description.to_string());
        }
        let url = format!("{base}/app/api/projects/{}/documents", project.id);
        let response = client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.as_u16() != 201 {
            return Err(anyhow!(
                "document upload failed: {status}: {}",
                first_line(&text)
            ));
        }
        println!("{text}");
        Ok(())
    })
    .await
}

/// An HTTP client that does **not** follow redirects, so a handler's `303`
/// (the notation lifecycle POSTs redirect on success) reads as success
/// rather than following the `Location` into an HTML page.
fn no_redirect_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build http client")
}

/// `navigator site projects list [--host h] [--json]`.
pub async fn projects_list(host: Option<&str>, json: bool) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let resp = reqwest::Client::new()
            .get(format!("{base}/app/projects.csv"))
            .bearer_auth(&token)
            .send()
            .await
            .context("GET /app/projects.csv")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("projects list failed: {status}"));
        }
        let rows = parse_csv(&body);
        print_projects(&rows, json)?;
        Ok(())
    })
    .await
}

/// Legacy project-open client — resolve a visible matter by
/// code, then verify the same bearer can load its lawyer workbench.
pub async fn matter_open(host: Option<&str>, project_code: &str) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let client = reqwest::Client::new();
        let path = format!("/app/projects/{project_code}");
        let resp = client
            .get(format!("{base}{path}"))
            .bearer_auth(&token)
            .send()
            .await
            .with_context(|| format!("GET {path}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "project `{project_code}` is not openable by this login (status {status}). \
                 The server reported: {}",
                first_line(&body),
            ));
        }
        println!(
            "{} {}",
            palette::dim("opened matter"),
            palette::highlight(project_code),
        );
        println!("{} {base}{path}", palette::dim("workbench:"));
        Ok(())
    })
    .await
}

/// `navigator site notation create <template-code> --project <code> --client-email …`
/// — open a notation on an **already-existing** matter and surface the
/// notation id. Every notation hangs on a pre-existing Project (the matter
/// is a deliberate prior step, `navigator db project create`), so `--project`
/// is required: this resolves the human-facing matter **code** to the
/// Project id, then posts to the project-scoped create route
/// (`POST /app/projects/<project-code>/notations/new`). The template is read
/// from the Project's git repo when authored there, else from the bundled
/// firm catalog. Leaves the questionnaire ready for the site intake flow.
pub async fn notation_create(
    host: Option<&str>,
    template: &str,
    client_email: &str,
    project_code: &str,
) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        // No-redirect client so we read the 303 `Location` (the step URL)
        // rather than following it into the walker's HTML.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build http client")?;
        let url = format!("{base}/app/projects/{project_code}/notations/new");
        let form: Vec<(&str, &str)> =
            vec![("template_code", template), ("client_email", client_email)];
        let resp = client
            .post(&url)
            .bearer_auth(&token)
            .form(&form)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        if status.as_u16() != 303 {
            // The walker re-renders its form (200) on a bad email/template;
            // anything but the redirect means no notation was created.
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "notation create did not start the questionnaire (status {status}). \
                 The server reported: {}",
                first_line(&body),
            ));
        }
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let notation_id = location
            .trim_start_matches("/app/lawyer/notations/")
            .trim_end_matches("/step");
        println!(
            "{} {}",
            palette::dim("created notation; questionnaire ready — notation"),
            palette::highlight(notation_id),
        );
        println!(
            "{}",
            palette::dim(format!(
                "continue this notation in the live site; notation id: {notation_id}"
            )),
        );
        Ok(())
    })
    .await
}

/// Legacy intake client — walk the notation's questionnaire one
/// question at a time over the same `/app/lawyer/notations/:id/step`
/// route the browser POSTs, reading each question's metadata from the
/// `?format=json` branch. Interactive by default (prompts at the
/// terminal); non-interactive when `--answer` / `--select` / `--person`
/// flags are supplied, consuming scalar answers in order, question-qualified
/// picker selections for candidate-backed questions, and the people rows
/// for the first `people_list` question.
pub async fn intake_answer(
    host: Option<&str>,
    notation_id: Uuid,
    answers: Vec<String>,
    selections: Vec<String>,
    persons: Vec<String>,
    transcript: Option<&Path>,
) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let client = reqwest::Client::new();
        // A transcript pre-fills the walk: batch coverage runs server-side and
        // seeds proposed answers the walk then offers as defaults. It's an
        // input mode of the same walk, so the interactive/scripted loop below
        // runs afterward regardless.
        if let Some(path) = transcript {
            post_transcript_coverage(&client, &base, &token, notation_id, path).await?;
        }
        let interactive = answers.is_empty() && selections.is_empty() && persons.is_empty();
        // Fail fast on a malformed `--person` before touching the server.
        let parsed_persons: Vec<Vec<(String, String)>> = persons
            .iter()
            .map(|s| crate::intake::parse_person(s))
            .collect::<Result<_>>()?;
        let mut answer_queue: VecDeque<String> = answers.into();
        let mut selection_queue: VecDeque<ScriptedSelection> = selections
            .iter()
            .map(|selection| parse_scripted_selection(selection))
            .collect::<Result<_>>()?;
        let mut persons_consumed = false;
        let mut answered = 0u32;

        loop {
            let step = fetch_step(&client, &base, &token, notation_id).await?;
            let Some(question) = step.question else {
                ensure_no_unused_selections(&selection_queue)?;
                print_questionnaire_complete(notation_id, answered);
                break;
            };

            let fields: Vec<(String, String)> =
                if store::question_registry::answer_type_is_aggregate(&question.answer_type) {
                    let rows = if interactive {
                        read_people_list(&question)?
                    } else {
                        if persons_consumed {
                            return Err(anyhow!(
                            "question `{}` is a people_list but every --person row was already \
                             consumed by an earlier one; this matter has more than one — answer \
                             it interactively",
                            question.code,
                        ));
                        }
                        persons_consumed = true;
                        crate::intake::people_list_fields(&parsed_persons)
                    };
                    rows
                } else if interactive {
                    prompt_scalar_fields(&question)?
                } else if question.has_candidates() {
                    let selection = selection_queue.pop_front().ok_or_else(|| {
                        anyhow!(
                            "ran out of --select values at picker question `{}` ({})",
                            question.code,
                            question.prompt,
                        )
                    })?;
                    scripted_picker_selection_fields(&question, &selection)?
                } else {
                    answer_queue
                        .pop_front()
                        .ok_or_else(|| {
                            anyhow!(
                                "ran out of --answer values at question `{}` ({})",
                                question.code,
                                question.prompt,
                            )
                        })
                        .map(|value| vec![("value".to_string(), value)])?
                };

            let resp = client
                .post(format!("{base}/app/lawyer/notations/{notation_id}/step"))
                .bearer_auth(&token)
                .form(&fields)
                .send()
                .await
                .context("POST step")?;
            let status = resp.status();
            if !status.is_success() && !status.is_redirection() {
                let body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "answering `{}` failed: {}",
                    question.code,
                    server_error(status, &body),
                ));
            }
            println!(
                "{} {}",
                palette::dim("answered"),
                palette::highlight(&question.code)
            );
            answered += 1;
        }
        Ok(())
    })
    .await
}

/// `navigator site notation approve <id>` — render + park the notation's
/// document (`POST …/approve-send`). The generic sibling of `retainer
/// approve`: it fills the bound packet (a formation's official Secretary-of-State form,
/// or a retainer PDF) for attorney review. Idempotent server-side — a
/// re-approve once the PDF exists is a success, not an error.
pub async fn notation_approve(host: Option<&str>, notation_id: Uuid) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let resp = reqwest::Client::new()
            .post(format!(
                "{base}/app/lawyer/notations/{notation_id}/approve-send"
            ))
            .bearer_auth(&token)
            // `Content-Length: 0` for the same LB gotcha as `retainer approve`.
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .body(Vec::<u8>::new())
            .send()
            .await
            .context("POST approve-send")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("approve failed: {}", server_error(status, &body)));
        }
        let st = fetch_status(&base, &token, notation_id).await?;
        println!(
            "{} {} — state {} (document_ready {})",
            palette::dim("approved — notation"),
            palette::highlight(notation_id.to_string()),
            palette::highlight(&st.state),
            st.document_ready,
        );
        Ok(())
    })
    .await
}

/// `navigator site notation request-changes <id> --question <code> [--note …]`
/// — send a notation parked at `lawyer_review` back for changes. Records the
/// flagged question codes + note and routes `lawyer_review -> reask__client`
/// so a rejected review re-collects the wrong answers instead of dead-ending
/// (`POST …/request-changes`). Declining the matter outright is a separate
/// action; this one loops back to review after the answers are corrected.
pub async fn notation_request_changes(
    host: Option<&str>,
    notation_id: Uuid,
    questions: &[String],
    note: Option<&str>,
) -> ExitCode {
    run(async {
        if questions.is_empty() {
            return Err(anyhow!(
                "pass at least one --question <code> to flag for re-collection"
            ));
        }
        let (base, token) = resolve(host)?;
        // Dynamic checkbox-style fields: `q:<code>=on` per flagged question,
        // plus the optional reviewer note — the shape the web form posts.
        let mut form: Vec<(String, String)> = questions
            .iter()
            .map(|code| (format!("q:{code}"), "on".to_string()))
            .collect();
        if let Some(note) = note {
            form.push(("note".to_string(), note.to_string()));
        }
        let resp = no_redirect_client()?
            .post(format!(
                "{base}/app/lawyer/notations/{notation_id}/request-changes"
            ))
            .bearer_auth(&token)
            .form(&form)
            .send()
            .await
            .context("POST request-changes")?;
        let status = resp.status();
        if !(status.is_success() || status.is_redirection()) {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "request-changes failed: {}",
                server_error(status, &body)
            ));
        }
        println!(
            "{} {} — flagged {} for re-collection",
            palette::dim("sent back for changes — notation"),
            palette::highlight(notation_id.to_string()),
            palette::highlight(questions.join(", ")),
        );
        Ok(())
    })
    .await
}

/// `navigator site notation update <id> --answer <code=value>` — re-collect the
/// flagged answers on a notation parked at `reask__client` (lawyer on the
/// client's behalf) and resubmit for review (`POST …/reask`). The write is
/// gated server-side to the flagged set, and the corrected answers are what
/// the attorney re-reviews — answers and questions stay decoupled, so a
/// correction never re-walks the whole questionnaire.
pub async fn notation_update(
    host: Option<&str>,
    notation_id: Uuid,
    answers: &[String],
) -> ExitCode {
    run(async {
        if answers.is_empty() {
            return Err(anyhow!(
                "pass at least one --answer <code=value> to re-collect"
            ));
        }
        let mut form: Vec<(String, String)> = Vec::with_capacity(answers.len());
        for raw in answers {
            let (code, value) = raw
                .split_once('=')
                .ok_or_else(|| anyhow!("--answer must be `code=value`, got `{raw}`"))?;
            form.push((format!("a:{code}"), value.to_string()));
        }
        let (base, token) = resolve(host)?;
        let resp = no_redirect_client()?
            .post(format!("{base}/app/lawyer/notations/{notation_id}/reask"))
            .bearer_auth(&token)
            .form(&form)
            .send()
            .await
            .context("POST reask")?;
        let status = resp.status();
        if !(status.is_success() || status.is_redirection()) {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("update failed: {}", server_error(status, &body)));
        }
        let st = fetch_status(&base, &token, notation_id).await?;
        println!(
            "{} {} — state {}",
            palette::dim("re-collected and resubmitted — notation"),
            palette::highlight(notation_id.to_string()),
            palette::highlight(&st.state),
        );
        Ok(())
    })
    .await
}

/// `navigator site notation document <id> --out <path>` — download the
/// notation's rendered document (the filled packet) to a local file via
/// the participation-gated `…/documents/document` route, the same per-
/// notation PDF the review surface shows.
pub async fn notation_document(host: Option<&str>, notation_id: Uuid, out: &Path) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        // Follow redirects: a signed-URL storage backend 302s to the blob;
        // the FsStorage dev backend streams 200 through the app.
        let resp = reqwest::Client::new()
            .get(format!(
                "{base}/app/lawyer/notations/{notation_id}/documents/document"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .context("GET document")?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(anyhow!(
                "no rendered document for notation {notation_id} — answer the questionnaire \
                 (or `navigator site notation approve {notation_id}`) first"
            ));
        }
        if !status.is_success() {
            return Err(anyhow!("document download failed: {status}"));
        }
        let bytes = resp.bytes().await.context("read document bytes")?;
        std::fs::write(out, &bytes).with_context(|| format!("write {}", out.display()))?;
        println!(
            "{} {} ({} bytes)",
            palette::dim("wrote the filled packet to"),
            palette::highlight(out.display().to_string()),
            bytes.len(),
        );
        Ok(())
    })
    .await
}

/// Legacy retainer client — POST approve-send. This renders +
/// parks: the worker durably renders + persists the retainer PDF and the
/// workflow waits at `generate_pdf__retainer_pdf`. It does NOT send — the
/// binding envelope goes out only on the separate `retainer send`, after
/// the PDF is confirmed present.
pub async fn retainer_approve(host: Option<&str>, notation_id: Uuid) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let resp = reqwest::Client::new()
            .post(format!(
                "{base}/app/lawyer/notations/{notation_id}/approve-send"
            ))
            .bearer_auth(&token)
            // Force `Content-Length: 0`. The handler takes no form fields,
            // but a bodyless POST carries no length header, and GCP's HTTPS
            // load balancer rejects that with `411 Length Required` before
            // the request ever reaches the app. reqwest omits the header for
            // an empty body, so set it explicitly.
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .body(Vec::<u8>::new())
            .send()
            .await
            .context("POST approve-send")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("approve failed: {}", server_error(status, &body)));
        }
        // Read the authoritative post-state for the operator.
        let st = fetch_status(&base, &token, notation_id).await?;
        println!(
            "{} {} — state {} (document_ready {})",
            palette::dim("approved; worker rendering the retainer PDF — notation"),
            palette::highlight(notation_id.to_string()),
            palette::highlight(&st.state),
            st.document_ready,
        );
        println!(
            "{}",
            palette::dim(format!(
                "dispatch the envelope from the live site for notation {notation_id}"
            )),
        );
        Ok(())
    })
    .await
}

/// Legacy retainer client — POST the deliberate send. On prod this
/// emits exactly one real envelope, so it is a deliberate authenticated
/// human command (never an LLM-routable tool). Honors the readiness gate:
/// a `409` means the worker hasn't rendered the PDF yet — print the
/// server's reason and exit non-zero so the operator retries rather than
/// the command silently loops against a misconfigured worker.
pub async fn retainer_send(host: Option<&str>, notation_id: Uuid) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        // `Content-Length: 0` for the same LB gotcha as `retainer approve`.
        let resp = reqwest::Client::new()
            .post(format!("{base}/app/lawyer/notations/{notation_id}/send"))
            .bearer_auth(&token)
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .body(Vec::<u8>::new())
            .send()
            .await
            .context("POST send")?;
        let status = resp.status();
        if status.as_u16() == 409 {
            // Not yet: the PDF isn't rendered. Print the server's reason
            // verbatim and tell the operator to retry.
            let body = resp.text().await.unwrap_or_default();
            let reason = json_reason(&body).unwrap_or_else(|| "document not ready yet".to_string());
            return Err(anyhow!(
                "not ready to send: {reason}\n\
                 retry from the live site for notation {notation_id}"
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("send failed: {}", server_error(status, &body)));
        }
        let st = fetch_status(&base, &token, notation_id).await?;
        println!(
            "{} {} — state {}{}",
            palette::dim("sent for signature; notation"),
            palette::highlight(notation_id.to_string()),
            palette::highlight(&st.state),
            st.signature_request_id
                .as_deref()
                .map(|id| format!(" (signature request {id})"))
                .unwrap_or_default(),
        );
        Ok(())
    })
    .await
}

/// Legacy retainer client — print the notation's custom
/// clauses from the clause editor's `?format=json` branch.
pub async fn clause_list(host: Option<&str>, notation_id: Uuid, json: bool) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let resp = reqwest::Client::new()
            .get(format!(
                "{base}/app/lawyer/notations/{notation_id}/clauses?format=json"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .context("GET clauses (json)")?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(anyhow!("no notation {notation_id} on {base}"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "clause list failed: {}",
                server_error(status, &body)
            ));
        }
        let body = resp.text().await.unwrap_or_default();
        if json {
            println!("{body}");
            return Ok(());
        }
        let clauses: Vec<Clause> = serde_json::from_str(&body).context("parse clauses json")?;
        if clauses.is_empty() {
            println!("{}", palette::dim("no custom clauses on this notation"));
            return Ok(());
        }
        for c in &clauses {
            let provenance = if c.system_authored {
                palette::dim("[system draft]")
            } else {
                palette::dim("[lawyer]")
            };
            println!(
                "{} {} {}\n    {}",
                palette::highlight(format!("#{}", c.position)),
                provenance,
                palette::dim(c.id.to_string()),
                c.body.replace('\n', "\n    "),
            );
        }
        Ok(())
    })
    .await
}

/// Legacy retainer client — append one clause.
pub async fn clause_add(host: Option<&str>, notation_id: Uuid, body: &str) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let resp = reqwest::Client::new()
            .post(format!("{base}/app/lawyer/notations/{notation_id}/clauses"))
            .bearer_auth(&token)
            .form(&[("body", body)])
            .send()
            .await
            .context("POST clause add")?;
        clause_write_result(resp, notation_id, "added").await
    })
    .await
}

/// Legacy retainer client — replace a body.
pub async fn clause_edit(
    host: Option<&str>,
    notation_id: Uuid,
    clause_id: Uuid,
    body: &str,
) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let resp = reqwest::Client::new()
            .post(format!(
                "{base}/app/lawyer/notations/{notation_id}/clauses/{clause_id}/edit"
            ))
            .bearer_auth(&token)
            .form(&[("body", body)])
            .send()
            .await
            .context("POST clause edit")?;
        clause_write_result(resp, notation_id, "updated").await
    })
    .await
}

/// Shared handler for a clause add/edit response: the routes 303-redirect
/// back to the clause page on success.
async fn clause_write_result(resp: reqwest::Response, notation_id: Uuid, verb: &str) -> Result<()> {
    let status = resp.status();
    // The clause routes redirect (303/302) on success; a no-redirect client
    // is not used here, so reqwest follows it and we land on the 200 page.
    if !status.is_success() && status.as_u16() != 303 {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "clause {verb} failed: {}",
            server_error(status, &body)
        ));
    }
    println!(
        "{} {}",
        palette::dim(format!("clause {verb} on notation")),
        palette::highlight(notation_id.to_string()),
    );
    Ok(())
}

/// One row of the clause editor's `?format=json` body.
#[derive(Debug, serde::Deserialize)]
struct Clause {
    id: Uuid,
    position: i32,
    body: String,
    #[serde(default)]
    system_authored: bool,
}

/// `navigator site notation status <id>` — print the workflow state + the
/// signature request id from the review handler's JSON branch.
pub async fn notation_status(host: Option<&str>, notation_id: Uuid, json: bool) -> ExitCode {
    run(async {
        let (base, token) = resolve(host)?;
        let st = fetch_status(&base, &token, notation_id).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&st)?);
        } else {
            println!(
                "{} state {}{} (delivery {}, document_ready {})",
                palette::dim(format!("notation {notation_id}")),
                palette::highlight(&st.state),
                st.signature_request_id
                    .as_deref()
                    .map(|id| format!(", signature request {id}"))
                    .unwrap_or_default(),
                st.delivery.as_deref().unwrap_or("—"),
                st.document_ready,
            );
        }
        Ok(())
    })
    .await
}

/// The review handler's `?format=json` body.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct NotationStatus {
    state: String,
    #[serde(default)]
    signature_request_id: Option<String>,
    #[serde(default)]
    delivery: Option<String>,
    /// Whether the worker has rendered + persisted the document PDF — the
    /// gate `retainer send` honors. Defaults to `false` for an older
    /// server that doesn't yet emit the field.
    #[serde(default)]
    document_ready: bool,
}

/// Render a non-2xx server response into an actionable line. The app's
/// error routes answer with a JSON `{error, reason}` body (the council's
/// "no opaque 500" point); fall back to the first line of a plain-text
/// body when the response isn't that shape.
fn server_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let error = v.get("error").and_then(serde_json::Value::as_str);
        let reason = v.get("reason").and_then(serde_json::Value::as_str);
        match (error, reason) {
            (Some(e), Some(r)) => return format!("{status}: {e} — {r}"),
            (_, Some(r)) => return format!("{status}: {r}"),
            (Some(e), _) => return format!("{status}: {e}"),
            _ => {}
        }
    }
    format!("{status}: {}", first_line(body))
}

/// The `reason` field of a server JSON error body, if present.
fn json_reason(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

/// One step of the questionnaire walker's `?format=json` body.
#[derive(Debug, serde::Deserialize)]
struct StepResponse {
    /// The next question, or `None` once the questionnaire reaches END.
    #[serde(default)]
    question: Option<StepQuestion>,
}

/// The question metadata the walker shows for one step.
#[derive(Debug, serde::Deserialize)]
struct StepQuestion {
    code: String,
    prompt: String,
    answer_type: String,
    /// `(value, label)` choices for a `radio`; empty otherwise.
    #[serde(default)]
    choices: Vec<StepChoice>,
    /// DB-backed picker candidates for singular record/reference
    /// questions, empty for primitive and aggregate answers.
    #[serde(default)]
    candidates: Vec<StepCandidate>,
    /// A prior answer for this step — a value navigated back to, or a
    /// transcript-coverage proposal — surfaced as an Enter-to-accept default.
    #[serde(default)]
    prior_answer: Option<String>,
    /// The `source` of `prior_answer` (`extracted` for a transcript proposal,
    /// `lawyer`/`client` for a typed one); labels the default in the walk.
    #[serde(default)]
    prior_source: Option<String>,
}

impl StepQuestion {
    fn has_candidates(&self) -> bool {
        !self.candidates.is_empty()
    }
}

#[derive(Debug, serde::Deserialize)]
struct StepChoice {
    value: String,
    label: String,
}

#[derive(Debug, serde::Deserialize)]
struct StepCandidate {
    id: Uuid,
    name: String,
}

struct ScriptedSelection {
    question_code: String,
    raw: String,
}

/// GET the current questionnaire step as JSON.
async fn fetch_step(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    notation_id: Uuid,
) -> Result<StepResponse> {
    let resp = client
        .get(format!(
            "{base}/app/lawyer/notations/{notation_id}/step?format=json"
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("GET step (json)")?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Err(anyhow!("no notation {notation_id} on {base}"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("step failed: {}", server_error(status, &body)));
    }
    resp.json::<StepResponse>().await.context("parse step json")
}

/// The coverage summary `POST …/transcript` returns: which inquiries the
/// transcript covered (each with its proposed answer) and which it left as
/// gaps the walk will still ask.
#[derive(Debug, serde::Deserialize)]
struct CoverageSummary {
    #[serde(default)]
    covered: Vec<CoveredInquiry>,
    #[serde(default)]
    uncovered: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CoveredInquiry {
    code: String,
    proposed_answer: String,
}

/// POST a transcript file to the batch-coverage endpoint, then print what it
/// covered. The proposals are persisted server-side (`source = extracted`) and
/// surface as the walk's Enter-to-accept defaults — never auto-accepted.
async fn post_transcript_coverage(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    notation_id: Uuid,
    path: &Path,
) -> Result<()> {
    let transcript = std::fs::read_to_string(path)
        .with_context(|| format!("read transcript {}", path.display()))?;
    let resp = client
        .post(format!("{base}/app/lawyer/notations/{notation_id}/transcript"))
        .bearer_auth(token)
        .form(&[("transcript", transcript.as_str())])
        .send()
        .await
        .context("POST transcript")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "transcript coverage failed: {}",
            server_error(status, &body)
        ));
    }
    let summary = resp
        .json::<CoverageSummary>()
        .await
        .context("parse coverage summary")?;
    println!(
        "{} {} covered, {} to confirm",
        palette::dim("transcript:"),
        palette::highlight(summary.covered.len()),
        palette::highlight(summary.uncovered.len()),
    );
    for inquiry in &summary.covered {
        println!(
            "  {} {} {}",
            palette::dim("proposed"),
            palette::highlight(&inquiry.code),
            palette::dim(format!("→ {}", inquiry.proposed_answer)),
        );
    }
    Ok(())
}

/// Interactively read one scalar answer, showing a `radio`'s choices or a
/// record/reference pick-list when the server exposes candidates.
fn prompt_scalar_fields(question: &StepQuestion) -> Result<Vec<(String, String)>> {
    println!("{}", palette::highlight(&question.prompt));
    if question.has_candidates() {
        print_candidate_table(question);
    }
    if !question.choices.is_empty() {
        println!("{}", palette::dim("choices:"));
        for c in &question.choices {
            println!("  {} — {}", palette::highlight(&c.value), c.label);
        }
    }
    // A transcript-coverage proposal (or a prior answer) becomes an
    // Enter-to-accept default. For a picker the proposal is matched to a
    // candidate by name (Enter posts that row's id); for a choice question the
    // accepted string resolves to the canonical choice value, not its label.
    let prior = question.prior_answer.as_deref().filter(|d| !d.is_empty());
    let default_candidate = if question.has_candidates() {
        prior.and_then(|p| candidate_by_name(question, p))
    } else {
        None
    };
    let default_display = if question.has_candidates() {
        default_candidate.map(|c| c.name.as_str())
    } else {
        prior
    };
    if let Some(prev) = default_display {
        let tag = if question.prior_source.as_deref() == Some("extracted") {
            "proposed from transcript"
        } else {
            "prior answer"
        };
        println!(
            "  {} {} {}",
            palette::dim(tag),
            palette::highlight(prev),
            palette::dim("(press Enter to accept, or type a new answer)"),
        );
    }
    print!("{} ", palette::dim(format!("{}>", question.code)));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read answer from stdin")?;
    let value = line.trim().to_string();
    if question.has_candidates() {
        // Enter accepts a name-matched proposal; otherwise the typed number /
        // id / name resolves against the candidates as before.
        if value.is_empty() {
            if let Some(candidate) = default_candidate {
                return Ok(vec![("id".to_string(), candidate.id.to_string())]);
            }
        }
        picker_selection_fields(question, &value)
    } else {
        let raw = if value.is_empty() {
            prior.unwrap_or_default().to_string()
        } else {
            value
        };
        Ok(vec![(
            "value".to_string(),
            canonical_choice_value(question, raw),
        )])
    }
}

/// Match a picker proposal string to a candidate by name (case-insensitive) so
/// a transcript-extracted display value can be confirmed as its row id.
fn candidate_by_name<'a>(question: &'a StepQuestion, name: &str) -> Option<&'a StepCandidate> {
    question
        .candidates
        .iter()
        .find(|candidate| candidate.name.eq_ignore_ascii_case(name))
}

/// Resolve an answer for a choice question (`radio`/`yes_no`) to its canonical
/// stored value: an exact value match wins, else a case-insensitive value or
/// label match maps to the choice's value. A question with no choices (or an
/// off-list value) keeps the string as typed. Without this, confirming an
/// extracted proposal whose display label differs from the choice value (e.g.
/// `Yes` vs `yes`) would store the label.
fn canonical_choice_value(question: &StepQuestion, raw: String) -> String {
    if question.choices.iter().any(|choice| choice.value == raw) {
        return raw;
    }
    question
        .choices
        .iter()
        .find(|choice| {
            choice.value.eq_ignore_ascii_case(&raw) || choice.label.eq_ignore_ascii_case(&raw)
        })
        .map_or(raw, |choice| choice.value.clone())
}

fn print_candidate_table(question: &StepQuestion) {
    let mut table = Table::new();
    table.load_style(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header([
        Cell::new(palette::header("#")),
        Cell::new(palette::header("name")),
        Cell::new(palette::header("id")),
    ]);
    for (idx, candidate) in question.candidates.iter().enumerate() {
        table.add_row([
            Cell::new(palette::highlight(idx + 1)),
            Cell::new(&candidate.name),
            Cell::new(palette::dim(candidate.id)),
        ]);
    }
    println!("{table}");
}

fn picker_selection_fields(question: &StepQuestion, raw: &str) -> Result<Vec<(String, String)>> {
    let selected = select_candidate(question, raw.trim())?;
    Ok(vec![("id".to_string(), selected.id.to_string())])
}

fn parse_scripted_selection(raw: &str) -> Result<ScriptedSelection> {
    let (question_code, value) = raw.split_once('=').ok_or_else(|| {
        anyhow!("--select must be `question_code=value`, for example `country__of_birth=2`")
    })?;
    let question_code = question_code.trim();
    let value = value.trim();
    if question_code.is_empty() || value.is_empty() {
        return Err(anyhow!(
            "--select must include both a question code and a value, for example `country__of_birth=2`"
        ));
    }
    Ok(ScriptedSelection {
        question_code: question_code.to_string(),
        raw: value.to_string(),
    })
}

fn print_questionnaire_complete(notation_id: Uuid, answered: u32) {
    if answered == 0 {
        println!(
            "{}",
            palette::dim(format!("notation {notation_id} has no open questions"))
        );
    } else {
        println!(
            "{} {} ({answered} answered)",
            palette::dim("questionnaire complete — notation"),
            palette::highlight(notation_id.to_string()),
        );
    }
}

fn scripted_picker_selection_fields(
    question: &StepQuestion,
    selection: &ScriptedSelection,
) -> Result<Vec<(String, String)>> {
    if selection.question_code != question.code {
        return Err(anyhow!(
            "--select for question `{}` reached picker question `{}`; pass selections in questionnaire order and use the current question code",
            selection.question_code,
            question.code
        ));
    }
    picker_selection_fields(question, &selection.raw)
}

fn ensure_no_unused_selections(selection_queue: &VecDeque<ScriptedSelection>) -> Result<()> {
    if let Some(selection) = selection_queue.front() {
        return Err(anyhow!(
            "unused --select for question `{}`; questionnaire has no remaining picker questions",
            selection.question_code
        ));
    }
    Ok(())
}

fn select_candidate<'a>(question: &'a StepQuestion, raw: &str) -> Result<&'a StepCandidate> {
    if raw.is_empty() {
        return Err(anyhow!(
            "question `{}` needs a picker selection; enter a list number or row id",
            question.code
        ));
    }
    if let Ok(n) = raw.parse::<usize>() {
        return question.candidates.get(n.saturating_sub(1)).ok_or_else(|| {
            anyhow!(
                "selection {n} is out of range for question `{}` ({} option{})",
                question.code,
                question.candidates.len(),
                if question.candidates.len() == 1 {
                    ""
                } else {
                    "s"
                },
            )
        });
    }
    if let Ok(id) = Uuid::parse_str(raw) {
        return question
            .candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| {
                anyhow!(
                    "selection id {id} is not listed for question `{}`",
                    question.code
                )
            });
    }
    Err(anyhow!(
        "selection `{raw}` for question `{}` is not a list number or row id",
        question.code
    ))
}

/// Interactively read a `people_list` answer row by row; a blank name
/// ends the list. Returns the assembled `p{row}_{part}` form fields.
fn read_people_list(question: &StepQuestion) -> Result<Vec<(String, String)>> {
    println!("{}", palette::highlight(&question.prompt));
    println!(
        "{}",
        palette::dim("enter each person; a blank name ends the list")
    );
    let stdin = std::io::stdin();
    let mut rows: Vec<Vec<(String, String)>> = Vec::new();
    loop {
        print!("{}", palette::dim("name (blank to finish)> "));
        std::io::stdout().flush().ok();
        let mut name = String::new();
        stdin.lock().read_line(&mut name).context("read name")?;
        let name = name.trim().to_string();
        if name.is_empty() {
            break;
        }
        let mut row = vec![("name".to_string(), name)];
        for part in &crate::intake::PARTS[1..] {
            print!("{}", palette::dim(format!("{part}> ")));
            std::io::stdout().flush().ok();
            let mut value = String::new();
            stdin
                .lock()
                .read_line(&mut value)
                .with_context(|| format!("read {part}"))?;
            let value = value.trim().to_string();
            if !value.is_empty() {
                row.push(((*part).to_string(), value));
            }
        }
        rows.push(row);
    }
    Ok(crate::intake::people_list_fields(&rows))
}

async fn fetch_status(base: &str, token: &str, notation_id: Uuid) -> Result<NotationStatus> {
    let resp = reqwest::Client::new()
        .get(format!(
            "{base}/app/lawyer/notations/{notation_id}/review?format=json"
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("GET notation review (json)")?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Err(anyhow!("no notation {notation_id} on {base}"));
    }
    if !status.is_success() {
        return Err(anyhow!("notation status failed: {status}"));
    }
    resp.json::<NotationStatus>()
        .await
        .context("parse notation status json")
}

fn print_projects(rows: &[Vec<String>], json: bool) -> Result<()> {
    let Some((header, data)) = rows.split_first() else {
        // Not even a header line — empty body.
        if json {
            println!("[]");
        } else {
            println!("{}", palette::dim("no projects"));
        }
        return Ok(());
    };
    if json {
        let objects: Vec<serde_json::Map<String, serde_json::Value>> = data
            .iter()
            .map(|row| {
                header
                    .iter()
                    .zip(row.iter())
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect()
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&objects)?);
        return Ok(());
    }
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(header.iter().map(|h| Cell::new(palette::header(h))));
    for row in data {
        table.add_row(row.iter().map(Cell::new));
    }
    println!("{table}");
    println!("{}", palette::dim(format!("{} project(s)", data.len())));
    Ok(())
}

/// First non-empty line of a response body, trimmed — for terse error
/// reporting without dumping a whole HTML page.
fn first_line(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(empty response)")
        .chars()
        .take(200)
        .collect()
}

/// Minimal RFC 4180 reader: comma-separated fields, `\r\n` or `\n`
/// records, `"`-quoted fields with doubled internal quotes. Mirrors the
/// server's `admin_csv` writer so the round-trip is exact.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut field = String::new();
    let mut record = Vec::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut saw_any = false;

    while let Some(c) = chars.next() {
        saw_any = true;
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    record.push(std::mem::take(&mut field));
                }
                '\r' => { /* swallow; the '\n' ends the record */ }
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut record));
                }
                _ => field.push(c),
            }
        }
    }
    // Trailing record with no final newline.
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        rows.push(record);
    }
    let _ = saw_any;
    rows
}

/// Drive an async fallible command to an `ExitCode`, printing any error.
async fn run<F>(fut: F) -> ExitCode
where
    F: std::future::Future<Output = Result<()>>,
{
    match fut.await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("navigator: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::process::ExitCode;
    use std::sync::LazyLock;

    use super::{
        candidate_by_name, canonical_choice_value, clause_add, clause_edit, clause_list,
        document_upload, ensure_no_unused_selections, fetch_status, matter_open, notation_approve,
        notation_create, notation_document, notation_request_changes, notation_status,
        notation_update, parse_scripted_selection, picker_selection_fields, projects_list,
        retainer_approve, retainer_send, scripted_picker_selection_fields, select_candidate,
        CoverageSummary, StepQuestion, StepResponse,
    };
    use super::{fetch_step, first_line, json_reason, parse_csv, server_error};
    use crate::credentials::{self, Credentials, HostCredential};
    use uuid::Uuid;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static CREDENTIALS_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    struct CredentialsEnv {
        _dir: tempfile::TempDir,
        previous: Option<String>,
    }

    #[derive(Clone, Copy)]
    struct LawyerRouteIds {
        notation: Uuid,
        project: Uuid,
        clause: Uuid,
    }

    impl CredentialsEnv {
        fn new(base: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("navigator.json");
            let mut creds = Credentials::default();
            creds.set(
                base,
                HostCredential {
                    token: "test-token".into(),
                    person_email: Some("lawyer@neonlaw.com".into()),
                    role: Some("lawyer".into()),
                    expires_at: super::now_secs() + 3600,
                },
            );
            credentials::save(&path, &creds).unwrap();
            let previous = std::env::var("NAVIGATOR_CREDENTIALS_FILE").ok();
            std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", path);
            Self {
                _dir: dir,
                previous,
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_reports_an_expired_stored_token() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("navigator.json");
        let base = "https://live.example.com";
        let mut creds = Credentials::default();
        creds.set(
            base,
            HostCredential {
                token: "stale-token".into(),
                person_email: Some("lawyer@neonlaw.com".into()),
                role: Some("lawyer".into()),
                expires_at: super::now_secs() - 1,
            },
        );
        credentials::save(&path, &creds).unwrap();
        let previous = std::env::var("NAVIGATOR_CREDENTIALS_FILE").ok();
        std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", &path);

        let err = super::resolve(Some(base)).unwrap_err();

        match previous {
            Some(p) => std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", p),
            None => std::env::remove_var("NAVIGATOR_CREDENTIALS_FILE"),
        }

        assert!(
            err.to_string().contains("expired"),
            "expected an expired-token error, got: {err}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_upload_posts_the_required_kind() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let project_id = Uuid::now_v7();
        let document_id = Uuid::now_v7();

        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": project_id, "code": "acme"}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/app/api/projects/{project_id}/documents")))
            .and(body_json(serde_json::json!({
                "filename": "note.txt",
                "content_base64": "dGVzdCBkb2N1bWVudA==",
                "content_type": "text/plain",
                "kind": "unclassified",
                "visibility": "internal"
            })))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "document_id": document_id })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("note.txt");
        std::fs::write(&named, b"test document").unwrap();

        assert_eq!(
            document_upload(
                Some(server_uri.as_str()),
                "acme",
                &named,
                "unclassified",
                None,
                None,
                Some("text/plain"),
            )
            .await,
            ExitCode::SUCCESS
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_upload_refuses_an_unknown_matter_code() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);

        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": Uuid::now_v7(), "code": "acme"}
            ])))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("note.txt");
        std::fs::write(&named, b"test document").unwrap();

        assert_eq!(
            document_upload(
                Some(server_uri.as_str()),
                "not-a-matter",
                &named,
                "unclassified",
                None,
                None,
                None,
            )
            .await,
            ExitCode::from(2)
        );
    }

    async fn mount_project_routes(server: &MockServer, ids: LawyerRouteIds) {
        Mock::given(method("GET"))
            .and(path("/app/projects.csv"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "id,code,name,status\r\n{},acme,Acme,open\r\n",
                ids.project
            )))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/app/projects/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_string("matter workbench"))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn mount_notation_routes(server: &MockServer, ids: LawyerRouteIds) {
        Mock::given(method("POST"))
            .and(path("/app/projects/acme/notations/new"))
            .respond_with(ResponseTemplate::new(303).append_header(
                "location",
                format!("/app/lawyer/notations/{}/step", ids.notation),
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/app/lawyer/notations/{}/step", ids.notation)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "question": {"code": "q1", "prompt": "Question one", "answer_type": "text"}
            })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/app/lawyer/notations/{}/approve-send",
                ids.notation
            )))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/app/lawyer/notations/{}/documents/document",
                ids.notation
            )))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"pdf bytes".to_vec()))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/app/lawyer/notations/{}/send", ids.notation)))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/app/lawyer/notations/{}/request-changes",
                ids.notation
            )))
            .respond_with(ResponseTemplate::new(303).append_header(
                "location",
                format!("/app/lawyer/notations/{}/reask", ids.notation),
            ))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/app/lawyer/notations/{}/reask", ids.notation)))
            .respond_with(ResponseTemplate::new(303).append_header(
                "location",
                format!("/app/lawyer/notations/{}/review", ids.notation),
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/app/lawyer/notations/{}/clauses", ids.notation)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/app/lawyer/notations/{}/clauses", ids.notation)))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/app/lawyer/notations/{}/clauses/{}/edit",
                ids.notation, ids.clause
            )))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/app/lawyer/notations/{}/review", ids.notation)))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "state": "lawyer_review",
                "delivery": "embedded",
                "document_ready": true
            })))
            .mount(server)
            .await;
    }

    async fn exercise_project_commands(host: Option<&str>) {
        assert_eq!(projects_list(host, true).await, ExitCode::SUCCESS);
        assert_eq!(matter_open(host, "acme").await, ExitCode::SUCCESS);
    }

    async fn exercise_notation_commands(host: Option<&str>, server_uri: &str, ids: LawyerRouteIds) {
        assert_eq!(
            notation_create(host, "memo__contract_review", "libra@example.com", "acme",).await,
            ExitCode::SUCCESS
        );
        let client = reqwest::Client::new();
        assert!(fetch_step(&client, server_uri, "test-token", ids.notation)
            .await
            .unwrap()
            .question
            .is_some());
        assert_eq!(
            notation_approve(host, ids.notation).await,
            ExitCode::SUCCESS
        );
        let out = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            notation_document(host, ids.notation, out.path()).await,
            ExitCode::SUCCESS
        );
        assert_eq!(std::fs::read(out.path()).unwrap(), b"pdf bytes");
        assert_eq!(
            retainer_approve(host, ids.notation).await,
            ExitCode::SUCCESS
        );
        assert_eq!(retainer_send(host, ids.notation).await, ExitCode::SUCCESS);
        assert_eq!(
            clause_list(host, ids.notation, true).await,
            ExitCode::SUCCESS
        );
        assert_eq!(
            clause_add(host, ids.notation, "custom clause").await,
            ExitCode::SUCCESS
        );
        assert_eq!(
            clause_edit(host, ids.notation, ids.clause, "updated").await,
            ExitCode::SUCCESS
        );
        assert_eq!(
            notation_status(host, ids.notation, true).await,
            ExitCode::SUCCESS
        );
        assert_eq!(
            notation_request_changes(
                host,
                ids.notation,
                &["person__client".to_string()],
                Some("confirm the spelling"),
            )
            .await,
            ExitCode::SUCCESS
        );
        assert_eq!(
            notation_update(
                host,
                ids.notation,
                &["person__client=Libra Jones".to_string()],
            )
            .await,
            ExitCode::SUCCESS
        );
        assert_eq!(
            fetch_status(server_uri, "test-token", ids.notation)
                .await
                .unwrap()
                .state,
            "lawyer_review"
        );
    }

    impl Drop for CredentialsEnv {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", previous);
            } else {
                std::env::remove_var("NAVIGATOR_CREDENTIALS_FILE");
            }
        }
    }

    #[test]
    fn parses_plain_rows() {
        let csv = "id,name,status\r\n1,Aries,open\r\n2,Taurus,closed\r\n";
        let rows = parse_csv(csv);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec!["id", "name", "status"]);
        assert_eq!(rows[1], vec!["1", "Aries", "open"]);
        assert_eq!(rows[2], vec!["2", "Taurus", "closed"]);
    }

    #[test]
    fn parses_quoted_fields_with_commas_and_doubled_quotes() {
        // Mirrors admin_csv's writer: `hello, "world"` round-trips.
        let csv = "id,note\r\n1,\"hello, \"\"world\"\"\"\r\n";
        let rows = parse_csv(csv);
        assert_eq!(rows[1], vec!["1", "hello, \"world\""]);
    }

    #[test]
    fn header_only_body_yields_one_row() {
        let rows = parse_csv("id,name\r\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec!["id", "name"]);
    }

    #[test]
    fn empty_body_yields_no_rows() {
        assert!(parse_csv("").is_empty());
    }

    #[test]
    fn tolerates_a_missing_final_newline() {
        let rows = parse_csv("id,name\r\n1,Aries");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec!["1", "Aries"]);
    }

    #[test]
    fn preserves_empty_trailing_field() {
        // A row ending in a comma has a trailing empty field (e.g. a
        // project with no entity name).
        let rows = parse_csv("a,b,c\r\nx,y,\r\n");
        assert_eq!(rows[1], vec!["x", "y", ""]);
    }

    #[test]
    fn step_json_deserializes_picker_candidates() {
        let mexico = Uuid::now_v7();
        let france = Uuid::now_v7();
        let step: StepResponse = serde_json::from_value(serde_json::json!({
            "question": {
                "code": "country__of_birth",
                "prompt": "Where were you born?",
                "answer_type": "country",
                "choices": [],
                "candidates": [
                    {"id": mexico, "name": "Mexico"},
                    {"id": france, "name": "France"}
                ]
            }
        }))
        .unwrap();
        let question = step.question.unwrap();
        assert_eq!(question.candidates.len(), 2);
        assert_eq!(question.candidates[0].id, mexico);
        assert_eq!(question.candidates[0].name, "Mexico");
    }

    /// The step carries a transcript-coverage proposal as `prior_answer` /
    /// `prior_source` so the walk can offer it as a default (defaults to `None`
    /// on an older server that omits them).
    #[test]
    fn step_json_deserializes_prior_answer_fields() {
        let step: StepResponse = serde_json::from_value(serde_json::json!({
            "question": {
                "code": "custom_text__testator_name",
                "prompt": "Who is the testator?",
                "answer_type": "string",
                "prior_answer": "Jane Doe",
                "prior_source": "extracted"
            }
        }))
        .unwrap();
        let question = step.question.unwrap();
        assert_eq!(question.prior_answer.as_deref(), Some("Jane Doe"));
        assert_eq!(question.prior_source.as_deref(), Some("extracted"));
    }

    fn choice_question(choices: &serde_json::Value) -> StepQuestion {
        serde_json::from_value(serde_json::json!({
            "code": "custom_yes_no__recording_consent",
            "prompt": "Consent to record?",
            "answer_type": "yes_no",
            "choices": choices,
        }))
        .unwrap()
    }

    #[test]
    fn canonical_choice_value_maps_label_to_value() {
        // A yes/no whose stored value differs from its display label — the
        // extracted proposal is the label, but confirming must store the value.
        let q = choice_question(&serde_json::json!([
            {"value": "yes", "label": "Yes"},
            {"value": "no", "label": "No"},
        ]));
        // Exact value kept; label (any case) and value (any case) map to value.
        assert_eq!(canonical_choice_value(&q, "yes".into()), "yes");
        assert_eq!(canonical_choice_value(&q, "Yes".into()), "yes");
        assert_eq!(canonical_choice_value(&q, "YES".into()), "yes");
        assert_eq!(canonical_choice_value(&q, "No".into()), "no");
        // An off-list value is kept as typed (free-text fallback).
        assert_eq!(canonical_choice_value(&q, "maybe".into()), "maybe");
    }

    #[test]
    fn canonical_choice_value_passthrough_without_choices() {
        let q = choice_question(&serde_json::json!([]));
        assert_eq!(canonical_choice_value(&q, "Jane Doe".into()), "Jane Doe");
    }

    #[test]
    fn candidate_by_name_matches_case_insensitively() {
        let mexico = Uuid::now_v7();
        let step: StepResponse = serde_json::from_value(serde_json::json!({
            "question": {
                "code": "country__of_birth",
                "prompt": "Where were you born?",
                "answer_type": "country",
                "candidates": [{"id": mexico, "name": "Mexico"}],
            }
        }))
        .unwrap();
        let q = step.question.unwrap();
        assert_eq!(candidate_by_name(&q, "mexico").map(|c| c.id), Some(mexico));
        assert!(candidate_by_name(&q, "Brazil").is_none());
    }

    #[test]
    fn coverage_summary_deserializes_covered_and_gaps() {
        let summary: CoverageSummary = serde_json::from_value(serde_json::json!({
            "template_code": "sitting__transcript",
            "covered": [{"code": "custom_yes_no__recording_consent", "proposed_answer": "Yes"}],
            "uncovered": ["custom_text__note"],
        }))
        .unwrap();
        assert_eq!(summary.covered.len(), 1);
        assert_eq!(summary.covered[0].code, "custom_yes_no__recording_consent");
        assert_eq!(summary.covered[0].proposed_answer, "Yes");
        assert_eq!(summary.uncovered, vec!["custom_text__note".to_string()]);
    }

    #[test]
    fn picker_selection_accepts_list_number_or_id() {
        let mexico = Uuid::now_v7();
        let france = Uuid::now_v7();
        let step: StepResponse = serde_json::from_value(serde_json::json!({
            "question": {
                "code": "country__of_birth",
                "prompt": "Where were you born?",
                "answer_type": "country",
                "candidates": [
                    {"id": mexico, "name": "Mexico"},
                    {"id": france, "name": "France"}
                ]
            }
        }))
        .unwrap();
        let question = step.question.unwrap();

        assert_eq!(select_candidate(&question, "2").unwrap().id, france);
        assert_eq!(
            select_candidate(&question, &mexico.to_string()).unwrap().id,
            mexico
        );
        assert!(select_candidate(&question, "3").is_err());
        assert!(select_candidate(&question, "France").is_err());
    }

    #[test]
    fn picker_selection_posts_the_selected_id_field() {
        let picked = Uuid::now_v7();
        let step: StepResponse = serde_json::from_value(serde_json::json!({
            "question": {
                "code": "entity__company",
                "prompt": "Which entity?",
                "answer_type": "entity",
                "candidates": [
                    {"id": picked, "name": "Bright Star Ventures LLC"}
                ]
            }
        }))
        .unwrap();
        let question = step.question.unwrap();

        assert_eq!(
            picker_selection_fields(&question, "1").unwrap(),
            vec![("id".to_string(), picked.to_string())]
        );
    }

    #[test]
    fn scripted_picker_selection_requires_question_code() {
        let parsed = parse_scripted_selection("country__of_birth=2").unwrap();
        assert_eq!(parsed.question_code, "country__of_birth");
        assert_eq!(parsed.raw, "2");

        assert!(parse_scripted_selection("2").is_err());
        assert!(parse_scripted_selection("country__of_birth=").is_err());
        assert!(parse_scripted_selection("=2").is_err());
    }

    #[test]
    fn scripted_picker_selection_rejects_question_mismatch() {
        let picked = Uuid::now_v7();
        let step: StepResponse = serde_json::from_value(serde_json::json!({
            "question": {
                "code": "country__of_birth",
                "prompt": "Where were you born?",
                "answer_type": "country",
                "candidates": [
                    {"id": picked, "name": "Mexico"}
                ]
            }
        }))
        .unwrap();
        let question = step.question.unwrap();
        let selection = parse_scripted_selection("country__of_birth=1").unwrap();
        let stale = parse_scripted_selection("country__residence=1").unwrap();

        assert_eq!(
            scripted_picker_selection_fields(&question, &selection).unwrap(),
            vec![("id".to_string(), picked.to_string())]
        );
        assert!(scripted_picker_selection_fields(&question, &stale).is_err());
    }

    #[test]
    fn scripted_picker_selection_rejects_leftovers() {
        let mut selections = VecDeque::new();
        assert!(ensure_no_unused_selections(&selections).is_ok());

        selections.push_back(parse_scripted_selection("country__of_birth=1").unwrap());
        assert!(ensure_no_unused_selections(&selections).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_commands_call_lawyer_routes() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let host = Some(server_uri.as_str());
        let ids = LawyerRouteIds {
            notation: Uuid::now_v7(),
            project: Uuid::now_v7(),
            clause: Uuid::now_v7(),
        };
        mount_project_routes(&server, ids).await;
        mount_notation_routes(&server, ids).await;

        exercise_project_commands(host).await;
        exercise_notation_commands(host, &server_uri, ids).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_open_refuses_codes_outside_the_visible_project_list() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let visible_project = Uuid::now_v7();
        Mock::given(method("GET"))
            .and(path("/app/projects.csv"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "id,code,name,status\r\n{visible_project},acme,Acme,open\r\n"
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/app/projects/acme"))
            .respond_with(ResponseTemplate::new(200).set_body_string("matter workbench"))
            .expect(0)
            .mount(&server)
            .await;

        assert_eq!(
            matter_open(Some(server_uri.as_str()), "missing").await,
            ExitCode::from(2)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_open_surfaces_a_workbench_that_refuses_this_login() {
        // The code resolves to a visible project, but loading its workbench
        // fails (e.g. row-scoped 403), so `project open` reports the failure
        // instead of claiming the matter opened.
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let visible_project = Uuid::now_v7();
        Mock::given(method("GET"))
            .and(path("/app/projects.csv"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "id,code,name,status\r\n{visible_project},acme,Acme,open\r\n"
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/app/projects/acme"))
            .respond_with(ResponseTemplate::new(403).set_body_string("not for you"))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            matter_open(Some(server_uri.as_str()), "acme").await,
            ExitCode::from(2)
        );
    }

    /// `retainer send` against a notation whose packet has not rendered yet
    /// answers `409`, and that is the one branch that must not read as a
    /// generic failure: the operator's next move is to retry, so the server's
    /// own reason and the retry command both have to reach them.
    #[tokio::test(flavor = "current_thread")]
    async fn sending_before_the_packet_renders_reports_the_reason_and_the_retry() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let notation = Uuid::now_v7();

        Mock::given(method("POST"))
            .and(path(format!("/app/lawyer/notations/{notation}/send")))
            .respond_with(ResponseTemplate::new(409).set_body_string(
                r#"{"error":"not ready","reason":"the retainer packet has not rendered"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            retainer_send(Some(server_uri.as_str()), notation).await,
            ExitCode::from(2)
        );
    }

    /// A `409` with no JSON reason still has to say something actionable
    /// rather than print an empty tail, so the handler supplies its own
    /// wording.
    #[tokio::test(flavor = "current_thread")]
    async fn a_409_without_a_json_reason_still_names_the_problem() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let notation = Uuid::now_v7();

        Mock::given(method("POST"))
            .and(path(format!("/app/lawyer/notations/{notation}/send")))
            .respond_with(ResponseTemplate::new(409).set_body_string("<html>gateway noise</html>"))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            retainer_send(Some(server_uri.as_str()), notation).await,
            ExitCode::from(2)
        );
    }

    /// Any other failing status takes the ordinary `server_error` path, which
    /// is a different branch from the 409 above.
    #[tokio::test(flavor = "current_thread")]
    async fn a_failing_send_reports_the_server_error() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::new(&server_uri);
        let notation = Uuid::now_v7();

        Mock::given(method("POST"))
            .and(path(format!("/app/lawyer/notations/{notation}/send")))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_string(r#"{"reason":"the signature provider refused"}"#),
            )
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            retainer_send(Some(server_uri.as_str()), notation).await,
            ExitCode::from(2)
        );
    }

    /// The council's "no opaque 500" rule lives in `server_error`: when a
    /// route answers with the `{error, reason}` shape, both halves reach the
    /// terminal, because the reason is the part that tells someone what to do
    /// next.
    #[test]
    fn a_json_error_body_renders_the_error_and_its_reason() {
        assert_eq!(
            server_error(
                reqwest::StatusCode::CONFLICT,
                r#"{"error":"already approved","reason":"the notation left review"}"#,
            ),
            "409 Conflict: already approved — the notation left review"
        );
    }

    /// Either field alone still beats printing the raw body.
    #[test]
    fn a_json_error_body_carrying_one_field_renders_that_field() {
        assert_eq!(
            server_error(
                reqwest::StatusCode::FORBIDDEN,
                r#"{"reason":"you do not participate on this matter"}"#,
            ),
            "403 Forbidden: you do not participate on this matter"
        );
        assert_eq!(
            server_error(
                reqwest::StatusCode::BAD_REQUEST,
                r#"{"error":"unknown step"}"#
            ),
            "400 Bad Request: unknown step"
        );
    }

    /// A body that parses as JSON but carries neither field is not a
    /// recognized error shape, so it takes the plain-text path rather than
    /// silently reporting the status with no detail at all.
    #[test]
    fn a_json_body_with_neither_field_falls_back_to_the_body_text() {
        assert_eq!(
            server_error(reqwest::StatusCode::NOT_FOUND, r#"{"detail":"nope"}"#),
            r#"404 Not Found: {"detail":"nope"}"#
        );
    }

    /// An HTML error page must not reach the terminal whole — a proxy 502
    /// answers with a document, not a sentence.
    #[test]
    fn a_non_json_error_body_is_reduced_to_its_first_real_line() {
        assert_eq!(
            server_error(
                reqwest::StatusCode::BAD_GATEWAY,
                "\n\n  <html><head><title>502 Bad Gateway</title></head>\n<body>more markup</body>\n",
            ),
            "502 Bad Gateway: <html><head><title>502 Bad Gateway</title></head>"
        );
    }

    #[test]
    fn json_reason_reads_the_field_and_tolerates_every_other_shape() {
        assert_eq!(
            json_reason(r#"{"reason":"the packet has not rendered yet"}"#).as_deref(),
            Some("the packet has not rendered yet")
        );
        // Present-but-wrong field, unparseable body, and empty body all mean
        // "no reason to show", so the caller falls back to its own wording.
        assert!(json_reason(r#"{"error":"no reason here"}"#).is_none());
        assert!(json_reason(r#"{"reason":404}"#).is_none());
        assert!(json_reason("<html>not json at all</html>").is_none());
        assert!(json_reason("").is_none());
    }

    #[test]
    fn first_line_skips_leading_blank_lines_and_trims() {
        assert_eq!(
            first_line("\n\n   the real line  \nsecond\n"),
            "the real line"
        );
    }

    /// A body with nothing printable in it is named rather than rendered as
    /// an empty string, so the error does not read as a blank.
    #[test]
    fn an_empty_body_is_named() {
        assert_eq!(first_line(""), "(empty response)");
        assert_eq!(first_line("\n   \n\t\n"), "(empty response)");
    }

    /// The 200-character cap is what stops a whole minified page from
    /// scrolling the terminal, and it counts characters rather than bytes so
    /// a multi-byte body cannot be cut mid-character.
    #[test]
    fn a_long_first_line_is_capped_at_two_hundred_characters() {
        assert_eq!(first_line(&"x".repeat(500)).chars().count(), 200);

        let wide = first_line(&"é".repeat(500));
        assert_eq!(wide.chars().count(), 200);
        assert_eq!(wide, "é".repeat(200));
    }
}
