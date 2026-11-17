//! Bounded local diagnostics. Callers provide only static event names and aggregate
//! numbers/booleans, never service bodies, filenames, IDs, URLs, or session headers.
//! One file is rewritten to prune old entries; no files outside the project are deleted.

use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 10 * 1024 * 1024;
const HISTORY_SECONDS: u64 = 30 * 24 * 60 * 60;
static LAST_PRUNE_DAY: Mutex<Option<u64>> = Mutex::new(None);

/// Keep the newest valid lines within both the age window and byte budget.
/// Malformed lines are discarded, making a partially written last line harmless.
fn retained(text: &str, now: u64, budget: usize) -> String {
    let mut used = 0;
    let mut lines = Vec::new();
    for line in text.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(stamp) = value["time"].as_u64() else {
            continue;
        };
        if stamp < now.saturating_sub(HISTORY_SECONDS) || stamp > now {
            continue;
        }
        if used + line.len() + 1 > budget {
            break;
        }
        used += line.len() + 1;
        lines.push(line);
    }
    lines.reverse();
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

/// Best-effort diagnostics must never stop a mutation or its verification.
/// Append normally; prune on startup, once per UTC day, and at the size limit.
/// Pruning at the size limit leaves half the budget free to avoid repeated rewrites.
pub fn record(root: &Path, event: &'static str, fields: Value) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let line = json!({"time":now,"event":event,"data":fields}).to_string() + "\n";
    if line.len() > 4096 {
        return;
    }
    let Ok(mut last_day) = LAST_PRUNE_DAY.lock() else {
        return;
    };
    let dir = root.join("reports");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("diagnostic.log");
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let full = size.saturating_add(line.len() as u64) > MAX_BYTES as u64;
    if *last_day != Some(now / 86400) || full {
        let Ok(text) = std::fs::read_to_string(&path).or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(String::new())
            } else {
                Err(e)
            }
        }) else {
            return;
        };
        let budget = if full {
            MAX_BYTES / 2
        } else {
            MAX_BYTES - line.len()
        };
        if std::fs::write(&path, retained(&text, now, budget)).is_err() {
            return;
        }
        *last_day = Some(now / 86400);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_retention_obeys_age_size_and_invalid_line_limits() {
        let now = HISTORY_SECONDS + 100;
        let old = json!({"time":99}).to_string() + "\n";
        let first = json!({"time":100}).to_string() + "\n";
        let last = json!({"time":now}).to_string() + "\n";
        let text = old + &first + "broken\n" + &last;
        assert_eq!(retained(&text, now, MAX_BYTES), first + &last);
        assert_eq!(retained(&text, now, last.len()), last);
        assert_eq!(retained(&text, now, 1), "");
        assert_eq!(retained(&text, now + HISTORY_SECONDS + 1, MAX_BYTES), "");
    }
}
