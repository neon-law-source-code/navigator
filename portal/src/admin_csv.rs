//! CSV export helpers for `/app/admin/*.csv` and `/app/projects.csv` endpoints.
//!
//! Writes RFC 4180 — fields are wrapped in `"` only when they
//! contain `,`, `"`, `\n`, or `\r`; internal `"` is doubled. Rows
//! end with `\r\n`. We don't pull a csv crate for this — the
//! escaping rules are 30 lines and adding a dep for it would buy
//! us nothing.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Serializable CSV payload. `headers` is one row of column names;
/// `rows` is every data row in the same column order. `filename` is
/// the suggested `Content-Disposition` filename (no path traversal —
/// keep it a plain leaf like `"people.csv"`).
pub struct CsvBody {
    pub filename: &'static str,
    pub headers: Vec<&'static str>,
    pub rows: Vec<Vec<String>>,
}

impl CsvBody {
    /// Render the body into an RFC 4180 string. Pure — exposed for
    /// unit tests that don't want to round-trip through axum.
    #[must_use]
    pub fn to_csv_string(&self) -> String {
        let mut out = String::new();
        write_row(&mut out, self.headers.iter().copied());
        for row in &self.rows {
            write_row(&mut out, row.iter().map(String::as_str));
        }
        out
    }
}

impl IntoResponse for CsvBody {
    fn into_response(self) -> Response {
        let body = self.to_csv_string();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/csv; charset=utf-8"),
        );
        // `filename` is &'static so this never fails — but use
        // try_from anyway so a future caller passing a runtime
        // string can't accidentally inject a CRLF into the header.
        if let Ok(disposition) = HeaderValue::try_from(format!(
            "attachment; filename=\"{}\"",
            self.filename.replace('"', "")
        )) {
            headers.insert(header::CONTENT_DISPOSITION, disposition);
        }
        (StatusCode::OK, headers, body).into_response()
    }
}

fn write_row<'a, I: Iterator<Item = &'a str>>(out: &mut String, fields: I) {
    let mut first = true;
    for field in fields {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&escape_field(field));
    }
    out.push_str("\r\n");
}

/// Quote a field per RFC 4180. Wraps in `"` and doubles internal
/// `"` when the value contains a delimiter, a quote, or a line
/// break; otherwise returns the original string unchanged.
#[must_use]
pub fn escape_field(field: &str) -> String {
    let needs_quoting = field.contains([',', '"', '\n', '\r']);
    if !needs_quoting {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len() + 2);
    out.push('"');
    for ch in field.chars() {
        if ch == '"' {
            out.push('"');
            out.push('"');
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::{escape_field, CsvBody};
    use axum::body::to_bytes;
    use axum::response::IntoResponse;

    #[test]
    fn plain_field_passes_through_unquoted() {
        assert_eq!(escape_field("hello"), "hello");
        assert_eq!(escape_field(""), "");
    }

    #[test]
    fn field_with_comma_gets_quoted() {
        assert_eq!(escape_field("a,b"), "\"a,b\"");
    }

    #[test]
    fn field_with_internal_quote_is_doubled_inside_quotes() {
        assert_eq!(escape_field("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn field_with_newline_gets_quoted() {
        assert_eq!(escape_field("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(escape_field("a\r\nb"), "\"a\r\nb\"");
    }

    #[test]
    fn csv_body_renders_headers_then_rows_with_crlf() {
        let body = CsvBody {
            filename: "x.csv",
            headers: vec!["id", "name"],
            rows: vec![
                vec!["1".into(), "Aries".into()],
                vec!["2".into(), "Taurus".into()],
            ],
        };
        assert_eq!(body.to_csv_string(), "id,name\r\n1,Aries\r\n2,Taurus\r\n");
    }

    #[test]
    fn csv_body_quotes_commas_and_quotes_in_row_values() {
        let body = CsvBody {
            filename: "x.csv",
            headers: vec!["id", "note"],
            rows: vec![vec!["1".into(), "hello, \"world\"".into()]],
        };
        assert_eq!(
            body.to_csv_string(),
            "id,note\r\n1,\"hello, \"\"world\"\"\"\r\n",
        );
    }

    #[test]
    fn csv_body_with_no_rows_emits_header_line_only() {
        let body = CsvBody {
            filename: "x.csv",
            headers: vec!["id", "name"],
            rows: vec![],
        };
        assert_eq!(body.to_csv_string(), "id,name\r\n");
    }

    #[tokio::test]
    async fn into_response_sets_text_csv_content_type_and_disposition() {
        let resp = CsvBody {
            filename: "people.csv",
            headers: vec!["id"],
            rows: vec![vec!["1".into()]],
        }
        .into_response();
        assert_eq!(resp.status(), 200);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap();
        assert_eq!(ct, "text/csv; charset=utf-8");
        let disp = resp
            .headers()
            .get(axum::http::header::CONTENT_DISPOSITION)
            .unwrap();
        assert_eq!(disp, "attachment; filename=\"people.csv\"");
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"id\r\n1\r\n");
    }

    #[tokio::test]
    async fn into_response_strips_double_quotes_from_filename_to_prevent_header_injection() {
        let resp = CsvBody {
            filename: "weird\"file.csv",
            headers: vec!["id"],
            rows: vec![],
        }
        .into_response();
        let disp = resp
            .headers()
            .get(axum::http::header::CONTENT_DISPOSITION)
            .unwrap();
        // Inner quote stripped so the surrounding `"..."` stays well-formed.
        assert_eq!(disp, "attachment; filename=\"weirdfile.csv\"");
    }
}
