use chrono::Utc;
use serde_json::json;

use crate::adapters::database::Database;
use crate::error::AppError;

const MAX_DIAGNOSTIC_LINE_BYTES: usize = 256 * 1024;
type DiagnosticTail = std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>;

enum DiagnosticRecord {
    Line(Vec<u8>),
    Omitted,
}

/// Drain owned stderr with bounded records, sanitizing before retaining text.
/// Oversized records are omitted whole: truncation could split a secret field.
/// This is not a wire parser; stdout/RPC keeps its fail-closed framing policy.
pub(crate) fn read_stderr_tail(
    stderr: impl std::io::Read,
    tail: DiagnosticTail,
    keep: usize,
    sanitize: fn(&str) -> String,
) {
    let mut reader = std::io::BufReader::new(stderr);
    while let Ok(Some(record)) = diagnostic_record(&mut reader) {
        let text = match record {
            DiagnosticRecord::Line(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            DiagnosticRecord::Omitted => "[diagnóstico excessivo omitido]".to_owned(),
        };
        let sanitized = sanitize(text.trim_end());
        if sanitized.is_empty() {
            continue;
        }
        let Ok(mut lines) = tail.lock() else {
            return;
        };
        lines.push_back(sanitized);
        while lines.len() > keep {
            lines.pop_front();
        }
    }
}

fn diagnostic_record(
    reader: &mut impl std::io::BufRead,
) -> std::io::Result<Option<DiagnosticRecord>> {
    use std::io::{BufRead, Read};
    let mut bytes = Vec::new();
    if reader
        .take((MAX_DIAGNOSTIC_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?
        == 0
    {
        return Ok(None);
    }
    if bytes.len() <= MAX_DIAGNOSTIC_LINE_BYTES {
        return Ok(Some(DiagnosticRecord::Line(bytes)));
    }
    if bytes.last() != Some(&b'\n') {
        reader.skip_until(b'\n')?;
    }
    Ok(Some(DiagnosticRecord::Omitted))
}

pub fn record(
    database: &Database,
    operation_id: Option<&str>,
    profile_id: Option<&str>,
    event_name: &str,
    mut fields: serde_json::Value,
) -> Result<(), AppError> {
    // Sanitize at the persistence boundary. Callers should still avoid
    // passing secrets, but a future diagnostic field cannot accidentally
    // place a credential in SQLite.
    sanitize_export(&mut fields);
    let mut payload = match fields {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    payload.insert("at".to_owned(), json!(Utc::now().to_rfc3339()));
    database.save_diagnostic(
        operation_id,
        profile_id,
        event_name,
        &serde_json::Value::Object(payload),
    )
}

pub fn sanitize_error(error: &str) -> String {
    let value = redact_json_fields(error);
    let value = redact_bearer_tokens(&value);
    redact_urls_in_text(&value).chars().take(500).collect()
}

fn redact_urls_in_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(index) = find_url_start(remaining) {
        output.push_str(&remaining[..index]);
        let tail = &remaining[index..];
        let end = tail
            .find(|character: char| character.is_whitespace() || "\"'<>".contains(character))
            .unwrap_or(tail.len());
        let (candidate, suffix) = trim_url_punctuation(&tail[..end]);
        let mut sanitized = candidate.to_owned();
        strip_url_details(&mut sanitized);
        output.push_str(&sanitized);
        output.push_str(suffix);
        remaining = &tail[end..];
    }
    output.push_str(remaining);
    output
}

fn find_url_start(value: &str) -> Option<usize> {
    [value.find("https://"), value.find("http://")]
        .into_iter()
        .flatten()
        .min()
}

fn trim_url_punctuation(value: &str) -> (&str, &str) {
    let end = value.trim_end_matches([',', ';', ')', '}', ']']).len();
    value.split_at(end)
}

pub fn sanitize_export(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                if is_secret_key(key) {
                    *nested = serde_json::Value::String("[redacted]".to_owned());
                } else {
                    sanitize_export(nested);
                }
            }
        }
        serde_json::Value::Array(values) => values.iter_mut().for_each(sanitize_export),
        serde_json::Value::String(text) => {
            *text = redact_json_fields(text);
            *text = redact_bearer_tokens(text);
            *text = redact_urls_in_text(text);
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "accesstoken",
        "refreshtoken",
        "idtoken",
        "clientsecret",
        "cookie",
        "authorization",
        "authurl",
        "oauthcode",
    ]
    .contains(&normalized.as_str())
}

fn strip_url_details(text: &mut String) {
    let Ok(mut parsed) = url::Url::parse(text) else {
        return;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    *text = parsed.into();
}

const SECRET_FIELD_NAMES: &[&str] = &[
    "access_token",
    "accesstoken",
    "refresh_token",
    "refreshtoken",
    "id_token",
    "idtoken",
    "client_secret",
    "clientsecret",
    "authorization",
    "cookie",
    "auth_url",
    "authurl",
    "oauth_code",
    "oauthcode",
    "code",
];

fn redact_json_fields(value: &str) -> String {
    let mut sanitized = value.to_owned();
    loop {
        let Some((start, end)) = next_secret_value(&sanitized) else {
            return sanitized;
        };
        sanitized.replace_range(start..end, "\"[redacted]\"");
    }
}

fn next_secret_value(value: &str) -> Option<(usize, usize)> {
    let lower = value.to_ascii_lowercase();
    let mut earliest = None;
    for field in SECRET_FIELD_NAMES {
        let needle = format!("\"{field}\"");
        let mut search_from = 0;
        while let Some(relative_start) = lower[search_from..].find(&needle) {
            let key_start = search_from + relative_start;
            let after_key = key_start + needle.len();
            let Some(colon_offset) = lower[after_key..].find(':') else {
                break;
            };
            let value_start = after_key + colon_offset + 1;
            let value_tail = value[value_start..].trim_start();
            let whitespace = value[value_start..].len() - value_tail.len();
            let mut stream =
                serde_json::Deserializer::from_str(value_tail).into_iter::<serde_json::Value>();
            let Some(Ok(parsed)) = stream.next() else {
                search_from = after_key;
                continue;
            };
            let start = value_start + whitespace;
            let end = start + stream.byte_offset();
            if parsed == serde_json::Value::String("[redacted]".to_owned()) {
                search_from = end;
                continue;
            }
            let is_earlier = match earliest {
                Some((current, _)) => start < current,
                None => true,
            };
            if is_earlier {
                earliest = Some((start, end));
            }
            break;
        }
    }
    earliest
}

fn redact_bearer_tokens(value: &str) -> String {
    const PREFIX: &str = "bearer ";
    const REDACTED: &str = "[redacted]";

    let mut sanitized = value.to_owned();
    let mut search_from = 0;
    loop {
        let lower = sanitized[search_from..].to_ascii_lowercase();
        let Some(relative_start) = lower.find(PREFIX) else {
            return sanitized;
        };
        let token_start = search_from + relative_start + PREFIX.len();
        if sanitized[token_start..].starts_with(REDACTED) {
            search_from = token_start + REDACTED.len();
            continue;
        }
        let token_end = sanitized[token_start..]
            .find(|character: char| character.is_whitespace() || "\"',;)}]".contains(character))
            .map_or(sanitized.len(), |offset| token_start + offset);
        if token_end == token_start {
            search_from = token_start;
            continue;
        }
        sanitized.replace_range(token_start..token_end, REDACTED);
        search_from = token_start + REDACTED.len();
    }
}

#[cfg(test)]
mod tests {
    use super::{record, sanitize_error, sanitize_export};
    use crate::adapters::database::Database;
    use serde_json::json;

    #[test]
    fn diagnostic_boundary_preserves_next_record_and_lossy_final_text() {
        use super::{diagnostic_record, DiagnosticRecord, MAX_DIAGNOSTIC_LINE_BYTES};
        use std::io::BufReader;
        let mut input = vec![b'x'; MAX_DIAGNOSTIC_LINE_BYTES];
        input.extend_from_slice(b"\nnormal\n\xff-final");
        let mut reader = BufReader::new(input.as_slice());

        assert!(matches!(
            diagnostic_record(&mut reader).expect("omission"),
            Some(DiagnosticRecord::Omitted)
        ));
        assert!(matches!(
            diagnostic_record(&mut reader).expect("next record"),
            Some(DiagnosticRecord::Line(bytes)) if bytes == b"normal\n"
        ));
        let tail = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        super::read_stderr_tail(reader, tail.clone(), 20, sanitize_error);
        assert_eq!(tail.lock().expect("tail").back().unwrap(), "�-final");
    }

    #[test]
    fn diagnostic_limit_includes_lf_and_oversized_eof_never_retains_a_fragment() {
        use super::{diagnostic_record, DiagnosticRecord, MAX_DIAGNOSTIC_LINE_BYTES};
        use std::io::{BufReader, Read};
        let mut exact = vec![b'x'; MAX_DIAGNOSTIC_LINE_BYTES - 1];
        exact.push(b'\n');
        assert!(matches!(
            diagnostic_record(&mut BufReader::new(exact.as_slice())).expect("exact limit"),
            Some(DiagnosticRecord::Line(bytes)) if bytes == exact
        ));
        let oversized = std::io::repeat(b'x').take(3 * 1024 * 1024);
        let tail = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        super::read_stderr_tail(oversized, tail.clone(), 24, sanitize_error);
        assert_eq!(
            tail.lock()
                .expect("tail")
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["[diagnóstico excessivo omitido]"]
        );
    }

    #[test]
    fn export_removes_tokens_and_oauth_query_strings() {
        let mut value = json!({
            "access_token": "secret",
            "nested": { "refreshToken": "also-secret" },
            "url": "https://login.example/callback?code=sensitive&state=sensitive#fragment"
        });
        sanitize_export(&mut value);
        assert_eq!(value["access_token"], "[redacted]");
        assert_eq!(value["nested"]["refreshToken"], "[redacted]");
        assert_eq!(value["url"], "https://login.example/callback");
    }

    #[test]
    fn error_text_never_keeps_oauth_query_parameters() {
        let sanitized = sanitize_error(
            "callback https://login.example/callback?code=secret&state=also-secret, failed",
        );
        assert_eq!(sanitized, "callback https://login.example/callback, failed");
    }

    #[test]
    fn error_redaction_handles_camel_case_and_escaped_secret_values() {
        let sanitized = sanitize_error(
            r#"{"accessToken":"secret,with}punctuation","refreshToken":"another-secret","authUrl":"https://login.example/callback?code=secret"}"#,
        );
        assert!(!sanitized.contains("secret,with}punctuation"));
        assert!(!sanitized.contains("another-secret"));
        assert_eq!(
            sanitized,
            r#"{"accessToken":"[redacted]","refreshToken":"[redacted]","authUrl":"[redacted]"}"#
        );
    }

    #[test]
    fn export_redacts_embedded_json_inside_error_strings() {
        let mut value = json!({
            "message": r#"{"authorization":"Bearer secret-value"}"#
        });
        sanitize_export(&mut value);
        assert_eq!(value["message"], r#"{"authorization":"[redacted]"}"#);
    }

    #[test]
    fn export_removes_query_strings_from_urls_embedded_in_text() {
        let mut value = json!({
            "message": "callback https://login.example/callback?code=secret&state=also-secret"
        });
        sanitize_export(&mut value);
        assert_eq!(value["message"], "callback https://login.example/callback");
    }

    #[test]
    fn repeated_secret_fields_and_bearer_tokens_are_all_redacted() {
        let sanitized = sanitize_error(
            r#"{"accessToken":"first"} {"accessToken":"second"} Authorization: Bearer third.secret"#,
        );

        assert!(!sanitized.contains("first"));
        assert!(!sanitized.contains("second"));
        assert!(!sanitized.contains("third.secret"));
        assert_eq!(sanitized.matches("[redacted]").count(), 3);
    }

    #[test]
    fn record_sanitizes_before_writing_to_sqlite() {
        let fixture = tempfile::tempdir().expect("fixture");
        let database = Database::open(fixture.path()).expect("database");

        record(
            &database,
            None,
            None,
            "test_event",
            json!({
                "accessToken": "must-not-reach-sqlite",
                "message": "callback https://login.example/callback?code=secret"
            }),
        )
        .expect("record");

        let events = database.diagnostic_export().expect("diagnostics");
        assert_eq!(events[0]["payload"]["accessToken"], "[redacted]");
        assert_eq!(
            events[0]["payload"]["message"],
            "callback https://login.example/callback"
        );
    }
}
