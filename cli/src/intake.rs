//! Pure helpers for the retired terminal intake flow — parsing the
//! `--person 'name=…,street=…'` flag and assembling the `people_list`
//! widget's `p{row}_{part}` form fields. The HTTP orchestration (walking
//! the server's `?format=json` steps and posting answers) lives in
//! [`crate::remote`]; these are the side-effect-free pieces so the
//! mapping is unit-testable without a server.

use anyhow::{anyhow, Result};

/// The ordered parts of one `people_list` row, mirroring
/// `portal::people_list_answer`'s `PARTS`. The server reads `p{row}_{part}`
/// for exactly these keys; anything else is dropped, so we reject unknown
/// keys here to catch a typo before it silently vanishes.
pub const PARTS: [&str; 7] = ["name", "title", "street", "city", "state", "zip", "country"];

/// Parse one `--person 'name=Libra,street=1 Main St,city=Las Vegas'`
/// spec into ordered `(part, value)` pairs. Comma-separates fields and
/// `=`-splits each into key/value; rejects an unknown key (the server
/// would silently drop it). Values may not contain a comma — a documented
/// limitation of the flag form; use the interactive walk for addresses
/// with commas.
pub fn parse_person(spec: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for field in spec.split(',') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| anyhow!("person field `{field}` must be key=value"))?;
        let key = key.trim();
        if !PARTS.contains(&key) {
            return Err(anyhow!(
                "unknown person field `{key}` (allowed: {})",
                PARTS.join(", "),
            ));
        }
        out.push((key.to_string(), value.trim().to_string()));
    }
    if !out.iter().any(|(k, _)| k == "name") {
        return Err(anyhow!(
            "--person needs at least a name= (e.g. --person 'name=Libra,street=1 Main St')",
        ));
    }
    Ok(out)
}

/// Assemble the `people_list` widget's form fields for a set of rows:
/// each row `r` contributes `p{r}_{part}=value` pairs, exactly what
/// `portal::people_list_answer::assemble` parses back into the JSON answer.
/// Empty rows (no fields) are skipped so a stray blank row doesn't shift
/// the indices.
#[must_use]
pub fn people_list_fields(rows: &[Vec<(String, String)>]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut row = 0usize;
    for person in rows {
        if person.is_empty() {
            continue;
        }
        for (part, value) in person {
            pairs.push((format!("p{row}_{part}"), value.clone()));
        }
        row += 1;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::{parse_person, people_list_fields};

    #[test]
    fn parse_person_reads_ordered_fields() {
        let p =
            parse_person("name=Libra,street=1 Main St,city=Las Vegas,state=NV,zip=89101").unwrap();
        assert_eq!(
            p,
            vec![
                ("name".to_string(), "Libra".to_string()),
                ("street".to_string(), "1 Main St".to_string()),
                ("city".to_string(), "Las Vegas".to_string()),
                ("state".to_string(), "NV".to_string()),
                ("zip".to_string(), "89101".to_string()),
            ],
        );
    }

    #[test]
    fn parse_person_rejects_unknown_key() {
        let err = parse_person("name=Libra,phone=555").unwrap_err();
        assert!(err.to_string().contains("unknown person field `phone`"));
    }

    #[test]
    fn parse_person_requires_a_name() {
        let err = parse_person("street=1 Main St").unwrap_err();
        assert!(err.to_string().contains("name="));
    }

    #[test]
    fn people_list_fields_indexes_rows_and_skips_empties() {
        let rows = vec![
            vec![
                ("name".to_string(), "Libra".to_string()),
                ("city".to_string(), "Las Vegas".to_string()),
            ],
            Vec::new(), // a blank row must not shift the surviving index
            vec![("name".to_string(), "Aries".to_string())],
        ];
        assert_eq!(
            people_list_fields(&rows),
            vec![
                ("p0_name".to_string(), "Libra".to_string()),
                ("p0_city".to_string(), "Las Vegas".to_string()),
                ("p1_name".to_string(), "Aries".to_string()),
            ],
        );
    }
}
