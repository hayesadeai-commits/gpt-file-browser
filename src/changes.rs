//! Reviewed soft-delete and restore operations with bounded deletion concurrency.
//!
//! Each file is checked against current service metadata before one mutation is
//! issued. Success requires readback evidence; an HTTP success alone is insufficient.
//! An uncertain result stops the batch and is never automatically retried.
//!
//! The browser-side sequence lives in `changes.js`. Reading all of Trash to confirm one
//! file costs a request per 50 trashed files, so it first asks Trash for that file alone;
//! `TrashLookup` carries what has been learned about those cheaper reads between files.

use crate::{AppEvent, inventory::FileRow, service};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use tao::event_loop::EventLoopProxy;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
/// The only supported mutations. Permanent deletion is deliberately absent.
pub enum Action {
    Trash,
    Restore,
}
#[derive(Clone, Serialize)]
/// Immutable selection shown for confirmation, bound to a single-use nonce.
pub struct Review {
    pub nonce: u64,
    pub action: Action,
    pub files: Vec<FileRow>,
}
#[derive(Clone, Serialize)]
/// User-visible receipt entry: verified, failed, uncertain, or skipped.
pub struct Outcome {
    pub id: String,
    pub name: String,
    pub status: String,
    pub message: String,
}
/// Separate bounded send and confirmation queues. current reserves every unfinished
/// file, including sends that have not replied, so the backlog cannot exceed the cap.
/// Session material in Confirmation is deliberately never serialized to receipts.
pub struct Job {
    pub action: Action,
    pub remaining: VecDeque<FileRow>,
    pub current: BTreeMap<String, FileRow>,
    pub confirmations: BTreeMap<String, Confirmation>,
    pub requests_sent: usize,
    pub outcomes: Vec<Outcome>,
    pub stop: bool,
}
pub const SEND_LIMIT: usize = 4;
pub const VERIFY_LIMIT: usize = 4;
pub const UNCONFIRMED_LIMIT: usize = 12;

#[derive(Clone)]
pub struct Confirmation {
    pub session: service::Session,
    pub sent: u64,
    pub mutation_status: Option<u16>,
    pub verifying: bool,
}

/// CDP can encode JavaScript integers as floating-point JSON (e.g. 200.0).
/// Accept only exact, nonnegative safe integers; never truncate malformed values.
pub fn browser_integer(value: &Value) -> Option<u64> {
    let n = value.as_f64()?;
    (n.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(&n) && n.fract() == 0.0)
        .then_some(n as u64)
}

/// What is known about confirming a file's state without reading all of Trash.
/// `Some(true)` works with this service, `Some(false)` does not, `None` is untested.
/// These hints only choose which read is tried first; they never count as evidence.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize)]
pub struct TrashLookup {
    /// Asking the Trash listing for one file by its ID.
    pub by_id: Option<bool>,
    /// Asking the Trash listing for one file by name.
    pub by_name: Option<bool>,
    /// Whether a file's own node still answers, as trashed, once it is in Trash.
    pub node: Option<bool>,
}

impl TrashLookup {
    /// Merge what a finished file reported. Anything but an explicit boolean is ignored.
    pub fn learn(&mut self, reported: &Value) {
        for (key, slot) in [
            ("by_id", &mut self.by_id),
            ("by_name", &mut self.by_name),
            ("node", &mut self.node),
        ] {
            if let Some(known) = reported.get(key).and_then(Value::as_bool) {
                *slot = Some(known);
            }
        }
    }
}

/// IDs reserved by confirmed work. Keep them out of selection without pretending
/// that a remote deletion already succeeded. Failed/skipped rows can then reappear.
pub fn reserved_ids(
    job: Option<&Job>,
    waiting: &VecDeque<Review>,
) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    if let Some(job) = job {
        ids.extend(
            job.current
                .values()
                .chain(job.remaining.iter())
                .map(|f| f.id.clone()),
        );
    }
    for review in waiting {
        ids.extend(review.files.iter().map(|f| f.id.clone()));
    }
    ids
}

/// Pop FIFO work only when no file remains active or pending in the current batch.
/// Keeping this decision pure makes serialization testable without a live WebView.
pub fn next_batch(job: Option<&Job>, waiting: &mut VecDeque<Review>) -> Option<Review> {
    if job.is_some_and(|j| !j.current.is_empty() || !j.remaining.is_empty()) {
        return None;
    }
    waiting.pop_front()
}

/// Recheck reservations at confirmation, including requests sent by a stale shell.
/// Bound queued snapshots independently of the per-batch selection limit.
pub fn validate_queue(
    review: &Review,
    job: Option<&Job>,
    waiting: &VecDeque<Review>,
) -> Result<(), String> {
    if job.is_some_and(|j| j.stop && !j.current.is_empty()) {
        return Err("Wait for the current batch to stop.".into());
    }
    if waiting.len() >= 20 {
        return Err("The queue already contains 20 waiting batches.".into());
    }
    let reserved = reserved_ids(job, waiting);
    if reserved.len() + review.files.len() > 10000 {
        return Err("The queue is limited to 10,000 files.".into());
    }
    if review.files.iter().any(|f| reserved.contains(&f.id)) {
        return Err("One or more selected files are already in a batch.".into());
    }
    Ok(())
}

impl Job {
    /// Convert a completed send into bounded pending verification without releasing its ID.
    pub fn request_sent(&mut self, id: &str, receipt: Confirmation) -> bool {
        if self.action != Action::Trash
            || !self.current.contains_key(id)
            || self.confirmations.contains_key(id)
        {
            return false;
        }
        self.confirmations.insert(id.into(), receipt);
        self.requests_sent += 1;
        true
    }
    /// Readback proceeds even after Stop or an error; only new sends are stopped.
    pub fn next_confirmation(&mut self) -> Option<(String, Confirmation)> {
        if self.confirmations.values().filter(|c| c.verifying).count() >= VERIFY_LIMIT {
            return None;
        }
        let (id, entry) = self.confirmations.iter_mut().find(|(_, c)| !c.verifying)?;
        entry.verifying = true;
        Some((id.clone(), entry.clone()))
    }
    /// Reserve a send slot and one position in the shared unfinished-file budget.
    /// Confirmed sends leave this pool but retain their reservation until readback finishes.
    pub fn take_next(&mut self) -> Option<FileRow> {
        if self.stop {
            while let Some(f) = self.remaining.pop_front() {
                self.outcomes.push(Outcome {
                    id: f.id,
                    name: f.name,
                    status: "skipped".into(),
                    message: "Not attempted after batch stopped.".into(),
                });
            }
            return None;
        }
        let capacity = if self.action == Action::Trash {
            SEND_LIMIT
        } else {
            1
        };
        if self.current.len().saturating_sub(self.confirmations.len()) >= capacity
            || self.current.len() >= UNCONFIRMED_LIMIT
        {
            return None;
        }
        while let Some(f) = self.remaining.pop_front() {
            // Native review already deduplicates IDs. Keep this defense at dispatch too.
            if self.current.contains_key(&f.id) || self.outcomes.iter().any(|o| o.id == f.id) {
                continue;
            }
            self.current.insert(f.id.clone(), f.clone());
            return Some(f);
        }
        None
    }
}

/// Reject blocked, shared, folder, special-artifact, and malformed-ID rows.
/// The browser script performs an additional live ownership/metadata check.
pub fn eligible(f: &FileRow) -> bool {
    !f.action_blocked
        && !f.folder
        && f.id.starts_with("libfile_")
        && f.id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        && f.access == "owned"
        && !["page", "site", "site_preview", "app_block"].contains(&f.artifact.as_str())
        && f.file_id.is_some()
}

/// Build the browser script for one file. The static preflight function is inserted first
/// and the request data last, so data (which includes the file name) is never scanned for
/// a placeholder and only ever lands where the script expects a JSON value.
fn script(
    session: &service::Session,
    row: &FileRow,
    action: Action,
    lookup: TrashLookup,
    phase: &str,
    confirmation: Option<&Confirmation>,
) -> String {
    let args = serde_json::json!({
        "file": row,
        "action": action,
        "authorization": session.authorization,
        "account": session.account,
        "lookup": lookup,
        "phase": phase,
        "sent": confirmation.map(|c|c.sent),
        "mutation_status": confirmation.and_then(|c|c.mutation_status),
    });
    include_str!("changes.js")
        .replace("__PREFLIGHT__", include_str!("preflight.js"))
        .replace("__ARGS__", &args.to_string())
}

/// Run preflight and one mutation; Trash readback uses its independent queue.
/// Restore retains the complete sequential flow. Generation is the active batch nonce.
/// These routes and query parameters were verified in Space's loaded action implementation.
/// Never request permanent deletion, and never retry an ambiguous mutation.
pub fn run(
    view: &wry::WebView,
    session: &service::Session,
    row: &FileRow,
    action: Action,
    generation: u64,
    lookup: TrashLookup,
    proxy: EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    if !eligible(row) {
        return Err(
            "Only owned Library files can be changed here; use ChatGPT for this item.".into(),
        );
    }
    let id = row.id.clone();
    let original_session = session.clone();
    let phase = if action == Action::Trash {
        "send"
    } else {
        "full"
    };
    service::evaluate_json(
        view,
        script(session, row, action, lookup, phase, None),
        move |r| {
            if r.as_ref().is_ok_and(|v| v["status"] == "sent") {
                let _ = proxy.send_event(AppEvent::ChangeSent(generation, id, original_session, r));
            } else {
                let _ = proxy.send_event(AppEvent::ChangeResult(generation, id, r));
            }
        },
    )
}

/// Verify only: the browser script's mutation block is inaccessible in this phase.
pub fn verify(
    view: &wry::WebView,
    row: &FileRow,
    receipt: &Confirmation,
    generation: u64,
    lookup: TrashLookup,
    proxy: EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    let id = row.id.clone();
    let expression = script(
        &receipt.session,
        row,
        Action::Trash,
        lookup,
        "verify",
        Some(receipt),
    );
    service::evaluate_json(view, expression, move |r| {
        let _ = proxy.send_event(AppEvent::ChangeResult(generation, id, r));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::parse_row;
    use serde_json::json;
    fn row(id: &str) -> FileRow {
        parse_row(
            &json!({"id":id,"name":"test.txt","file_id":"file_fixture","access_kind":"owned"}),
        )
        .unwrap()
    }
    #[test]
    fn cdp_numbers_accept_integral_floats_without_truncation() {
        for json in ["1700000000000", "1700000000000.0", "1.7e12"] {
            assert_eq!(
                browser_integer(&serde_json::from_str(json).unwrap()),
                Some(1_700_000_000_000)
            );
        }
        for json in ["null", "true", "-1", "200.5", "9007199254740992", "\"200\""] {
            assert_eq!(browser_integer(&serde_json::from_str(json).unwrap()), None);
        }
        assert_eq!(browser_integer(&serde_json::json!(200.0)), Some(200));
    }
    #[test]
    fn independent_pools_obey_backpressure_and_drain_after_stop() {
        let mut job = Job {
            action: Action::Trash,
            remaining: (0..20).map(|i| row(&format!("libfile_{i}"))).collect(),
            current: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: false,
        };
        let receipt = || Confirmation {
            session: service::Session {
                url: String::new(),
                authorization: "fixture-secret".into(),
                account: None,
            },
            sent: 123,
            mutation_status: Some(200),
            verifying: false,
        };
        // Four sends may overlap four readbacks; pending files retain reservations.
        for _ in 0..3 {
            let ids: Vec<_> = (0..SEND_LIMIT)
                .map(|_| job.take_next().unwrap().id)
                .collect();
            assert!(job.take_next().is_none());
            for id in ids {
                assert!(job.request_sent(&id, receipt()));
                assert!(!job.request_sent(&id, receipt()));
            }
            while job.next_confirmation().is_some() {}
        }
        assert_eq!(job.current.len(), UNCONFIRMED_LIMIT);
        assert_eq!(job.requests_sent, 12);
        assert_eq!(
            job.confirmations.values().filter(|c| c.verifying).count(),
            VERIFY_LIMIT
        );
        assert!(job.take_next().is_none());
        let id = job
            .confirmations
            .iter()
            .find(|(_, c)| c.verifying)
            .unwrap()
            .0
            .clone();
        job.confirmations.remove(&id);
        job.current.remove(&id);
        assert!(job.next_confirmation().is_some());
        assert!(job.take_next().is_some());
        assert!(job.take_next().is_none());
        job.stop = true;
        assert!(job.take_next().is_none());
        let id = job
            .confirmations
            .iter()
            .find(|(_, c)| c.verifying)
            .unwrap()
            .0
            .clone();
        job.confirmations.remove(&id);
        job.current.remove(&id);
        assert!(
            job.next_confirmation().is_some(),
            "Stopping sends must still drain queued verification"
        );
        assert_eq!(job.outcomes.len(), 7);
    }
    #[test]
    fn lookup_hints_merge_only_explicit_answers() {
        let mut lookup = TrashLookup::default();
        lookup.learn(&json!({"by_id":true,"by_name":null,"node":false,"other":true}));
        assert_eq!(
            lookup,
            TrashLookup {
                by_id: Some(true),
                by_name: None,
                node: Some(false)
            }
        );
        // A later file that learned nothing new, or returned no hints at all, changes nothing.
        lookup.learn(&json!({"by_id":null,"node":"yes"}));
        lookup.learn(&Value::Null);
        assert_eq!(lookup.by_id, Some(true));
        assert_eq!(lookup.node, Some(false));
        lookup.learn(&json!({"by_id":false}));
        assert_eq!(lookup.by_id, Some(false));
    }
    #[test]
    fn request_data_is_only_ever_inserted_as_a_json_value() {
        let template = include_str!("changes.js");
        for placeholder in ["__ARGS__", "__PREFLIGHT__"] {
            assert_eq!(template.matches(placeholder).count(), 1, "{placeholder}");
            assert!(!include_str!("preflight.js").contains(placeholder));
        }
        // A hostile file name must stay inside its JSON string: it cannot close a comment,
        // end the string, or be mistaken for a placeholder.
        let mut file = row("libfile_a");
        file.name = "*/ __PREFLIGHT__ \"; alert(1); /* __ARGS__.png".into();
        let session = service::Session {
            url: String::new(),
            authorization: "not-exported".into(),
            account: None,
        };
        let lookup = TrashLookup {
            by_id: Some(true),
            ..Default::default()
        };
        let built = script(&session, &file, Action::Trash, lookup, "full", None);
        let name = serde_json::to_string(&file.name).unwrap();
        assert_eq!(built.matches(&name).count(), 1);
        let (head, tail) = built.split_once(&name).unwrap();
        assert!(!head.contains("__PREFLIGHT__") && !tail.contains("__PREFLIGHT__"));
        assert!(!head.contains("__ARGS__") && !tail.contains("__ARGS__"));
        assert!(built.contains(r#""lookup":{"by_id":true,"by_name":null,"node":null}"#));
        assert!(built.contains("const a = {"));
        // The preflight comparison is embedded exactly once.
        assert_eq!(built.matches("return 'file version'").count(), 1);
        // Permanent deletion is never requested.
        assert!(built.contains("soft_delete: 'true'") && !built.contains("soft_delete: 'false'"));
        assert!(!built.contains("delete-batch"));
    }
    #[test]
    fn queued_batches_run_fifo_only_after_the_active_batch_finishes() {
        let mut job = Job {
            action: Action::Trash,
            remaining: vec![row("libfile_a")].into(),
            current: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: false,
        };
        let mut waiting = VecDeque::from([
            Review {
                nonce: 20,
                action: Action::Trash,
                files: vec![row("libfile_b")],
            },
            Review {
                nonce: 30,
                action: Action::Trash,
                files: vec![row("libfile_c")],
            },
        ]);
        assert!(next_batch(Some(&job), &mut waiting).is_none());
        job.take_next();
        assert!(next_batch(Some(&job), &mut waiting).is_none());
        assert_eq!(waiting.len(), 2);
        job.current.clear();
        assert_eq!(next_batch(Some(&job), &mut waiting).unwrap().nonce, 20);
        assert_eq!(next_batch(None, &mut waiting).unwrap().nonce, 30);
    }
    #[test]
    fn confirmed_batches_reserve_files_without_blocking_new_selections() {
        let mut job = Job {
            action: Action::Trash,
            remaining: vec![row("libfile_a"), row("libfile_b")].into(),
            current: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: false,
        };
        job.take_next();
        let waiting = VecDeque::from([Review {
            nonce: 2,
            action: Action::Trash,
            files: vec![row("libfile_c")],
        }]);
        let reserved = reserved_ids(Some(&job), &waiting);
        assert_eq!(reserved.len(), 3);
        assert!(
            validate_queue(
                &Review {
                    nonce: 3,
                    action: Action::Trash,
                    files: vec![row("libfile_d")]
                },
                Some(&job),
                &waiting
            )
            .is_ok()
        );
        assert!(
            validate_queue(
                &Review {
                    nonce: 4,
                    action: Action::Trash,
                    files: vec![row("libfile_a")]
                },
                Some(&job),
                &waiting
            )
            .is_err()
        );
        assert!(
            validate_queue(
                &Review {
                    nonce: 5,
                    action: Action::Trash,
                    files: vec![row("libfile_c")]
                },
                Some(&job),
                &waiting
            )
            .is_err()
        );
        job.current.clear();
        job.stop = true;
        job.take_next();
        assert!(reserved_ids(Some(&job), &VecDeque::new()).is_empty());
    }
    #[test]
    fn stopping_and_queue_limits_reject_new_confirmations() {
        let job = Job {
            action: Action::Trash,
            remaining: VecDeque::new(),
            current: BTreeMap::from([("libfile_a".into(), row("libfile_a"))]),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: true,
        };
        let review = Review {
            nonce: 1,
            action: Action::Trash,
            files: vec![row("libfile_b")],
        };
        assert!(validate_queue(&review, Some(&job), &VecDeque::new()).is_err());
        let waiting = (0..20)
            .map(|n| Review {
                nonce: n,
                action: Action::Trash,
                files: vec![row(&format!("libfile_{n}"))],
            })
            .collect();
        assert!(validate_queue(&review, None, &waiting).is_err());
    }
    #[test]
    fn only_owned_normal_files_can_be_trashed() {
        let mut f =
            parse_row(&json!({"id":"libfile_x","name":"A","file_id":"f-x","access_kind":"owned"}))
                .unwrap();
        assert!(eligible(&f));
        f.action_blocked = true;
        assert!(!eligible(&f));
        f.action_blocked = false;
        f.access = "shared".into();
        assert!(!eligible(&f));
        f.access = "owned".into();
        f.artifact = "page".into();
        assert!(!eligible(&f));
        f.artifact = "".into();
        f.id = "libfile_../../evil".into();
        assert!(!eligible(&f));
    }
    #[test]
    fn four_files_overlap_but_stop_drains_only_unstarted_work() {
        let mut j = Job {
            action: Action::Trash,
            remaining: vec![
                row("libfile_a"),
                row("libfile_b"),
                row("libfile_c"),
                row("libfile_d"),
                row("libfile_e"),
            ]
            .into(),
            current: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: false,
        };
        assert_eq!(j.take_next().unwrap().id, "libfile_a");
        assert_eq!(j.take_next().unwrap().id, "libfile_b");
        assert_eq!(j.take_next().unwrap().id, "libfile_c");
        assert_eq!(j.take_next().unwrap().id, "libfile_d");
        assert!(j.take_next().is_none());
        assert_eq!(j.current.len(), 4);
        // B may finish first without consuming A's reservation or opening a new batch.
        assert_eq!(j.current.remove("libfile_b").unwrap().id, "libfile_b");
        j.stop = true;
        assert!(j.take_next().is_none());
        assert!(j.current.contains_key("libfile_a"));
        assert_eq!(j.current.len(), 3);
        assert_eq!(j.outcomes[0].id, "libfile_e");
        assert_eq!(j.outcomes[0].status, "skipped");
        let mut waiting = VecDeque::from([Review {
            nonce: 9,
            action: Action::Trash,
            files: vec![row("libfile_d")],
        }]);
        assert!(next_batch(Some(&j), &mut waiting).is_none());
        j.current.remove("libfile_a");
        j.current.remove("libfile_d");
        assert!(next_batch(Some(&j), &mut waiting).is_none());
        j.current.remove("libfile_c");
        assert!(next_batch(Some(&j), &mut waiting).is_some());
    }
    #[test]
    fn restore_stays_serial_and_duplicate_ids_are_never_dispatched_twice() {
        let mut j = Job {
            action: Action::Restore,
            remaining: vec![row("libfile_a"), row("libfile_b")].into(),
            current: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: false,
        };
        assert!(j.take_next().is_some());
        assert!(j.take_next().is_none());
        j.current.clear();
        assert_eq!(j.take_next().unwrap().id, "libfile_b");
        j.action = Action::Trash;
        j.remaining = vec![row("libfile_b"), row("libfile_b")].into();
        assert!(j.take_next().is_none());
        assert_eq!(j.current.len(), 1);
    }
}
