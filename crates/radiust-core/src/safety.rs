//! Secret and terminal-control sanitization shared by bindings and reports.

use regex::{Captures, Regex};
use serde_json::{Map, Value};
use std::sync::LazyLock;
use url::Url;

static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https?://[^\s<>\"']+"#).expect("URL redaction expression is valid")
});
static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(Bearer\s+)[^\s\x00-\x1f]+").expect("Bearer redaction expression is valid")
});
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authorization\s*[:=]\s*)(?:(?:Bearer|Basic|Token)\s+)?[^\s\x00-\x1f]+")
        .expect("authorization redaction expression is valid")
});
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b((?:token|secret|password|passwd|authorization|(?:x[_-])?api[_-]?key|access[_-]?(?:key|token)|secret[_-]?key|credential|cookie|bearer|refresh[_-]?token|client[_-]?secret|(?:[a-z0-9]+[_-])*(?:signature|sig))\s*[:=]\s*)[^\s,;&]+",
    )
    .expect("secret assignment redaction expression is valid")
});

/// Redact credentials from URLs, authorization text, assignments, and controls.
pub fn safe_text(value: &str) -> String {
    let urls = URL.replace_all(value, |captures: &Captures<'_>| {
        redact_url(captures.get(0).map_or("", |value| value.as_str()))
    });
    let bearer = BEARER.replace_all(&urls, "$1[REDACTED]");
    let authorization = AUTHORIZATION.replace_all(&bearer, "$1[REDACTED]");
    let assignments = ASSIGNMENT.replace_all(&authorization, "$1[REDACTED]");
    let mut output = String::with_capacity(assignments.len());
    for character in assignments.chars() {
        let value = character as u32;
        if value < 0x20 || value == 0x7f || (0x80..=0x9f).contains(&value) {
            if value <= 0xff {
                output.push_str(&format!("\\x{value:02x}"));
            } else {
                output.push_str(&format!("\\u{{{value:x}}}"));
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// Recursively sanitize JSON values without changing non-string data.
pub fn safe_value(value: Value) -> Value {
    match value {
        Value::Object(values) => {
            let mut sanitized = Map::new();
            for (key, child) in values {
                let safe_key = safe_text(&key);
                let safe_child = if sensitive_key(&key) {
                    Value::String("[REDACTED]".into())
                } else {
                    safe_value(child)
                };
                sanitized.insert(safe_key, safe_child);
            }
            Value::Object(sanitized)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(safe_value).collect()),
        Value::String(value) => Value::String(safe_text(&value)),
        value => value,
    }
}

fn redact_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "[REDACTED URL]".into();
    };
    let username = url.set_username("");
    let password = url.set_password(None);
    let mut query = url.query_pairs().map(|(key, value)| {
        if sensitive_query_key(&key) {
            (key.into_owned(), "[REDACTED]".to_owned())
        } else {
            (key.into_owned(), value.into_owned())
        }
    });
    let pairs = query.by_ref().collect::<Vec<_>>();
    if !pairs.is_empty() {
        let encoded =
            url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
        url.set_query(Some(&encoded));
    }
    url.set_fragment(None);
    if username.is_err() || password.is_err() {
        return "[REDACTED URL]".into();
    }
    url.to_string()
}

fn sensitive_query_key(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "authorization",
        "api_key",
        "apikey",
        "access_key",
        "accesskey",
        "credential",
        "cookie",
        "bearer",
        "signature",
        "sig",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn sensitive_key(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "token"
            | "secret"
            | "password"
            | "passwd"
            | "authorization"
            | "api_key"
            | "api-key"
            | "x_api_key"
            | "x-api-key"
            | "apikey"
            | "access_key"
            | "access-key"
            | "access_token"
            | "access-token"
            | "secret_key"
            | "secret-key"
            | "credential"
            | "cookie"
            | "bearer"
            | "refresh_token"
            | "refresh-token"
            | "client_secret"
            | "client-secret"
    ) || lower.ends_with("_signature")
        || lower.ends_with("-signature")
        || lower.ends_with("_sig")
        || lower.ends_with("-sig")
}

#[cfg(test)]
mod tests {
    use super::{safe_text, safe_value};
    use serde_json::json;

    #[test]
    fn redacts_credentials_and_control_bytes_in_text() {
        let value = safe_text(
            "https://example.test/path?apiKey=abc&mode=raw Bearer abc authorization: Basic xyz token=secret \u{1b}",
        );
        assert!(!value.contains("abc"));
        assert!(!value.contains("xyz"));
        assert!(!value.contains("secret"));
        assert!(value.contains("apiKey=[REDACTED]"));
        assert!(value.contains("\\x1b"));
    }

    #[test]
    fn recursively_redacts_sensitive_values_but_keeps_safe_fields() {
        assert_eq!(
            safe_value(json!({
                "message": "token=private",
                "access_key": "private",
                "name": "radar",
                "nested": [{"cookie": "private", "count": 2}],
            })),
            json!({
                "message": "token=[REDACTED]",
                "access_key": "[REDACTED]",
                "name": "radar",
                "nested": [{"cookie": "[REDACTED]", "count": 2}],
            })
        );
    }
}
