//! Privacy-conscious diagnostic descriptions and browser URL boundaries.
//!
//! Reports describe types and selected field names, never scalar response values.
//! This is a structural debugging aid, not a general-purpose anonymizer: exported
//! reports should still be reviewed before sharing.

use serde::Serialize;
use serde_json::Value;
use url::Url;

pub const LIMIT: usize = 150;
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
/// Exportable structural metadata; request credentials are represented only as a boolean.
pub struct Observation {
    pub path: String,
    pub method: String,
    pub query: Vec<(String, String)>,
    pub authenticated_header: bool,
    pub status: i32,
    pub schema: String,
}

/// Retain query names and a narrow set of simple option values; redact other values.
pub fn query_shape(raw: &str) -> Vec<(String, String)> {
    Url::parse(raw)
        .ok()
        .map(|u| {
            u.query_pairs()
                .map(|(k, v)| {
                    let safe_value = if v.is_empty() {
                        "<empty>".into()
                    } else if [
                        "limit",
                        "offset",
                        "page_size",
                        "page",
                        "sort",
                        "sort_by",
                        "order",
                        "direction",
                        "include_deleted",
                        "include_archived",
                        "filter",
                        "source",
                        "include_folders",
                    ]
                    .contains(&k.as_ref())
                        && v.len() < 60
                        && v.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                    {
                        v.to_string()
                    } else {
                        "<present>".into()
                    };
                    (k.to_string(), safe_value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Select relevant HTTPS service metadata routes and replace dynamic path segments.
/// Exclude authentication, token, download, and content paths from inspection.
pub fn candidate(raw: &str) -> Option<String> {
    let u = Url::parse(raw).ok()?;
    if u.scheme() != "https" || u.host_str() != Some("chatgpt.com") {
        return None;
    }
    let parts: Vec<_> = u.path_segments()?.collect();
    if !parts
        .iter()
        .any(|s| matches!(*s, "library" | "drive" | "space" | "files"))
    {
        return None;
    }
    if parts.iter().any(|s| {
        ["auth", "token", "session", "download", "content"]
            .iter()
            .any(|x| s.to_ascii_lowercase().contains(x))
    }) {
        return None;
    }
    Some(format!(
        "/{}",
        parts
            .iter()
            .map(|s| {
                if !s.is_empty()
                    && s.len() < 40
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_')
                {
                    *s
                } else {
                    "{id}"
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    ))
}

/// Allow ChatGPT and its normal authentication destinations in the embedded browser.
/// This navigation policy is separate from the narrower authenticated-fetch policy.
pub fn allowed_navigation(raw: &str) -> bool {
    Url::parse(raw).ok().is_some_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| {
                h == "chatgpt.com"
                    || h == "openai.com"
                    || h.ends_with(".openai.com")
                    || h == "accounts.google.com"
            })
    })
}

/// Parse a bounded JSON response and return a bounded structural summary.
pub fn describe(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_BYTES {
        return Err("Response exceeds capture limit".into());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "Response is not supported JSON")?;
    let mut out = Vec::new();
    walk(&v, "$", 0, &mut out);
    Ok(out.join("\n"))
}

/// Describe types recursively with depth, field-count, and output-line limits.
/// Inspect only the first array element, never scalar values; unknown keys are redacted.
fn walk(v: &Value, path: &str, depth: usize, out: &mut Vec<String>) {
    if out.len() >= 180 {
        return;
    }
    let kind = match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    out.push(format!(
        "{path}: {kind}{}",
        v.as_array()
            .map(|a| format!(" ({} items)", a.len()))
            .unwrap_or_default()
    ));
    if depth >= 6 {
        return;
    }
    match v {
        Value::Object(o) => {
            for (k, child) in o.iter().take(40) {
                // Recognized schema names only. Dynamic keys can themselves contain private data.
                let record_field = path == "$.items[0]"
                    && k.len() < 64
                    && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && !["token", "cookie", "password", "secret", "authorization"]
                        .iter()
                        .any(|x| k.to_ascii_lowercase().contains(x));
                let safe = match k.as_str() {
                    "data" | "items" | "files" | "folders" | "results" | "entries" | "children"
                    | "id" | "file_id" | "name" | "filename" | "title" | "size" | "bytes"
                    | "size_bytes" | "file_size" | "created_at" | "updated_at" | "modified_at"
                    | "mime_type" | "type" | "kind" | "parent_id" | "folder_id" | "next_cursor"
                    | "cursor" | "has_more" | "total" | "count" | "limit" | "offset"
                    | "metadata" | "status" | "is_deleted" | "deleted_at" | "next"
                    | "pagination" | "permissions" | "owner" | "workspace_id" | "url" => k.as_str(),
                    _ if record_field => k.as_str(),
                    _ => "[other-key]",
                };
                walk(child, &format!("{path}.{safe}"), depth + 1, out);
            }
        }
        Value::Array(a) => {
            if let Some(child) = a.first() {
                walk(child, &format!("{path}[0]"), depth + 1, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values_and_dynamic_keys_are_not_exported() {
        let s = describe(br#"{"items":[{"name":"secret.zip","bytes":99991,"modified_at":"private-date"}],"next_cursor":"private-cursor","access_token":"secret-token","privatefilename":{"name":"x"}}"#).unwrap();
        for secret in [
            "secret.zip",
            "99991",
            "private-date",
            "private-cursor",
            "access_token",
            "secret-token",
            "privatefilename",
        ] {
            assert!(!s.contains(secret));
        }
        assert!(s.contains("$.items[0].bytes: number"));
        assert!(s.contains("$.next_cursor: string"));
    }
    #[test]
    fn only_service_metadata_paths() {
        assert!(
            candidate("https://chatgpt.com/backend-api/library/items?cursor=SECRET")
                .unwrap()
                .ends_with("library/items")
        );
        for raw in [
            "https://chatgpt.com.evil.test/library",
            "http://chatgpt.com/library",
            "https://chatgpt.com/backend-api/auth/session",
            "https://chatgpt.com/files/abc/content",
            "https://chatgpt.com/files/download",
        ] {
            assert!(candidate(raw).is_none());
        }
        assert!(
            !candidate("https://chatgpt.com/files/abc123?token=SECRET")
                .unwrap()
                .contains("SECRET")
        );
    }
    #[test]
    fn navigation_is_host_bounded() {
        assert!(allowed_navigation("https://auth.openai.com/login"));
        assert!(!allowed_navigation("https://openai.com.evil.test"));
        assert!(!allowed_navigation("file:///C:/Windows"));
    }
    #[test]
    fn malformed_and_oversized_json_fail() {
        assert!(describe(b"not json").is_err());
        assert!(describe(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
    #[test]
    fn output_is_bounded() {
        let s = describe(
            serde_json::to_string(&vec![serde_json::json!({"name":"hidden"}); 1000])
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        assert!(s.contains("1000 items"));
        assert!(s.lines().count() < 10);
    }
}
