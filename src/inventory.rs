//! Normalized file metadata and complete Library traversal.
//!
//! A scan walks root pages and discovered folders, deduplicating by stable ID.
//! Partial data may be displayed, but completeness is earned only when the task
//! queue is exhausted without invalid responses or pagination-limit violations.

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};

#[derive(Clone, Serialize, Debug)]
/// Shared metadata contract for the Rust coordinator and JavaScript table.
/// This is a snapshot, not permission to mutate: eligibility and live preflight run later.
pub struct FileRow {
    pub id: String,
    pub name: String,
    pub bytes: Option<u64>,
    pub modified: Option<String>,
    pub mime: String,
    pub parent: String,
    pub folder: bool,
    pub file_id: Option<String>,
    pub access: String,
    pub artifact: String,
    /// Local quarantine after an unverified action; never received as remote authority.
    pub action_blocked: bool,
}

#[derive(Clone, Debug)]
/// One root/folder listing request. A missing cursor means its first page.
pub struct PageTask {
    pub parent: Option<String>,
    pub cursor: Option<String>,
}

#[derive(Default)]
/// Traversal queue, deduplication state, and the current partial/complete snapshot.
pub struct Inventory {
    pub rows: BTreeMap<String, FileRow>,
    pub queue: VecDeque<PageTask>,
    visited: HashSet<(Option<String>, Option<String>)>,
    folders: HashSet<String>,
    pub pages: usize,
    pub complete: bool,
    pub error: Option<String>,
}

impl Inventory {
    /// Begin an incomplete scan with a single root request.
    pub fn start() -> Self {
        let mut s = Self::default();
        s.queue.push_back(PageTask {
            parent: None,
            cursor: None,
        });
        s
    }
    /// Take one unseen task. Only an empty queue marks the snapshot complete.
    /// Callers must accept each response before requesting the next task.
    pub fn next(&mut self) -> Result<Option<PageTask>, String> {
        if self.pages >= 10_000 && !self.queue.is_empty() {
            return Err("Stopped after 10,000 pages; inventory is incomplete.".into());
        }
        let Some(task) = self.queue.pop_front() else {
            self.complete = true;
            return Ok(None);
        };
        if !self
            .visited
            .insert((task.parent.clone(), task.cursor.clone()))
        {
            return Err("The service repeated a pagination cursor; refresh is required.".into());
        }
        Ok(Some(task))
    }
    /// Validate one response, merge owned-Library ID forms, and enqueue more work.
    /// Folder IDs are scheduled once; page cursors are checked again when consumed.
    pub fn accept(&mut self, task: &PageTask, body: &Value) -> Result<(), String> {
        let items = body
            .get("items")
            .and_then(Value::as_array)
            .ok_or("The listing format changed (items missing).")?;
        if !body
            .get("cursor")
            .is_some_and(|v| v.is_null() || v.is_string())
        {
            return Err("The listing format changed (cursor missing).".into());
        }
        for item in items {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or("An item has no stable ID.")?;
            // These are the actual owned Library ID forms observed in Space. External providers stay out.
            let folder = id.starts_with("libdir_");
            if !folder && !id.starts_with("libfile_") {
                continue;
            }
            if item.get("trashed_at").is_some_and(|v| !v.is_null()) {
                continue;
            }
            let row = parse_row(item)?;
            self.rows.insert(id.into(), row);
            if folder && self.folders.insert(id.into()) {
                // Finish current-folder pagination first; visit each discovered folder afterward.
                self.queue.push_back(PageTask {
                    parent: Some(id.into()),
                    cursor: None,
                });
            }
        }
        if let Some(c) = body
            .get("cursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
        {
            self.queue.push_front(PageTask {
                parent: task.parent.clone(),
                cursor: Some(c.into()),
            });
        }
        self.pages += 1;
        Ok(())
    }
}

// Missing pagination information is never proof that a listing is complete.
/// Require an explicit null or string cursor; an omitted key is a schema error.
pub fn page_cursor(body: &Value) -> Result<Option<String>, String> {
    match body.get("cursor") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(c)) if c.is_empty() => Ok(None),
        Some(Value::String(c)) => Ok(Some(c.clone())),
        _ => Err("Listing pagination format changed (cursor missing or invalid).".into()),
    }
}

/// Normalize node/listing field names into the UI contract.
/// Missing optional metadata stays unknown; a fresh scan resets action_blocked.
pub fn parse_row(item: &Value) -> Result<FileRow, String> {
    let id = item.get("id").and_then(Value::as_str).ok_or("Missing ID")?;
    let name = item
        .get("name")
        .or_else(|| item.get("file_name"))
        .and_then(Value::as_str)
        .ok_or("Missing filename")?;
    Ok(FileRow {
        action_blocked: false,
        id: id.into(),
        name: name.into(),
        bytes: item.get("file_size_bytes").and_then(Value::as_u64),
        modified: item
            .get("updated_at")
            .and_then(Value::as_str)
            .map(str::to_owned),
        mime: item
            .get("mime_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        parent: item
            .get("parent_directory_id")
            .or_else(|| item.get("directory_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        folder: id.starts_with("libdir_"),
        file_id: item
            .get("file_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        access: item
            .get("access_kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        artifact: item
            .get("library_artifact_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn listing_limit_allows_completion_but_not_an_extra_page() {
        let mut inv = Inventory::start();
        inv.pages = 10_000;
        assert!(inv.next().is_err());
        inv.queue.clear();
        assert!(inv.next().unwrap().is_none());
        assert!(inv.complete);
    }
    #[test]
    fn pagination_requires_an_explicit_end_marker() {
        assert!(page_cursor(&json!({"items":[]})).is_err());
        assert!(page_cursor(&json!({"cursor":false})).is_err());
        assert_eq!(page_cursor(&json!({"cursor":null})).unwrap(), None);
        assert_eq!(
            page_cursor(&json!({"cursor":"next"})).unwrap(),
            Some("next".into())
        );
    }
    #[test]
    fn traverses_pages_and_folders_without_duplicate_files() {
        let mut inv = Inventory::start();
        let first = inv.next().unwrap().unwrap();
        inv.accept(&first,&json!({"items":[{"id":"libdir_a","name":"Folder"},{"id":"libfile_a","name":"A","file_size_bytes":100},{"id":"external-gdrive:root","name":"Drive"}],"cursor":"next"})).unwrap();
        let next = inv.next().unwrap().unwrap();
        assert_eq!(next.cursor.as_deref(), Some("next"));
        inv.accept(
            &next,
            &json!({"items":[{"id":"libfile_a","name":"A","file_size_bytes":100}],"cursor":null}),
        )
        .unwrap();
        let folder = inv.next().unwrap().unwrap();
        assert_eq!(folder.parent.as_deref(), Some("libdir_a"));
        inv.accept(
            &folder,
            &json!({"items":[{"id":"libfile_b","name":"B","file_size_bytes":999}],"cursor":null}),
        )
        .unwrap();
        assert!(inv.next().unwrap().is_none());
        assert!(inv.complete);
        assert_eq!(inv.rows.len(), 3);
    }
    #[test]
    fn repeated_cursor_and_invalid_pages_never_complete() {
        let mut inv = Inventory::start();
        let t = inv.next().unwrap().unwrap();
        assert!(inv.accept(&t, &json!({"error":"denied"})).is_err());
        assert!(!inv.complete);
        inv.accept(&t, &json!({"items":[],"cursor":"x"})).unwrap();
        let t = inv.next().unwrap().unwrap();
        inv.accept(&t, &json!({"items":[],"cursor":"x"})).unwrap();
        assert!(inv.next().is_err());
        assert!(!inv.complete);
    }
}
