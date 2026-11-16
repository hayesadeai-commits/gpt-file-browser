//! Reviewed, sequential soft-delete and restore operations.
//!
//! Each file is checked against current service metadata before one mutation is
//! issued. Success requires readback evidence; an HTTP success alone is insufficient.
//! An uncertain result stops the batch and is never automatically retried.

use crate::{AppEvent, inventory::FileRow, service};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
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
/// Sequential batch state. current reserves the one in-flight file; stop prevents
/// new requests but lets the current request finish its verification.
pub struct Job {
    pub action: Action,
    pub remaining: VecDeque<FileRow>,
    pub current: Option<FileRow>,
    pub outcomes: Vec<Outcome>,
    pub stop: bool,
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
                .iter()
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
    if job.is_some_and(|j| j.current.is_some() || !j.remaining.is_empty()) {
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
    if job.is_some_and(|j| j.stop && j.current.is_some()) {
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
    /// Reserve the next file or drain the rest as skipped after a stop.
    /// Repeated calls while a file is active cannot duplicate a mutation.
    pub fn take_next(&mut self) -> Option<FileRow> {
        if self.current.is_some() {
            return None;
        }
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
        let f = self.remaining.pop_front()?;
        self.current = Some(f.clone());
        Some(f)
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

/// Run a single preflight, mutation, and verification sequence in ChatGPT.
/// The generation argument is the review nonce, not the inventory scan generation.
pub fn run(
    view: &wry::WebView,
    session: &service::Session,
    row: &FileRow,
    action: Action,
    generation: u64,
    proxy: EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    if !eligible(row) {
        return Err(
            "Only owned Library files can be changed here; use ChatGPT for this item.".into(),
        );
    }
    let args = serde_json::json!({"file":row,"action":action,"authorization":session.authorization,"account":session.account});
    let preflight = include_str!("preflight.js");
    // These routes and query parameters were verified in Space's loaded action implementation.
    // Never request permanent deletion, and never retry an ambiguous mutation.
    let script = format!(
        r#"(async()=>{{
      if(location.origin!=='https://chatgpt.com')throw Error('Sign-in required');
      const a={args}, f=a.file, base='/backend-api/files/library/files/'+encodeURIComponent(f.id);
      const headers={{Authorization:a.authorization}};if(a.account)headers['ChatGPT-Account-ID']=a.account;
      async function req(url,method='GET',body){{const h={{...headers}};if(body)h['Content-Type']='application/json';const r=await fetch(url,{{method,credentials:'include',headers:h,body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(30000)}});let b=null;try{{b=await r.json()}}catch{{}}return {{status:r.status,body:b}}}}
      function isTrashed(n){{return n&&n.trashed_at!=null}}
      let trashProblem=null;
      // Normal node lookup excludes trashed files. Walk the dedicated Trash listing instead.
      // The page bound matches the Trash browser; repeated/malformed cursors fail closed.
      async function findInTrash(){{let cursor=null;const seen=new Set();for(let page=0;page<1000;page++){{const r=await req('/backend-api/files/library','POST',{{limit:50,cursor,trashed_only:true,include_hidden_files:true}});if(r.status!==200||!Array.isArray(r.body?.items)||!Object.hasOwn(r.body,'cursor')||!(r.body.cursor===null||typeof r.body.cursor==='string')){{trashProblem='Trash response could not be verified (HTTP '+r.status+').';return null;}}const match=r.body.items.find(n=>n.id===f.id);if(match)return match;cursor=r.body.cursor;if(!cursor)return null;if(seen.has(cursor)){{trashProblem='Trash repeated a pagination cursor.';return null;}}seen.add(cursor)}}trashProblem='Trash verification exceeded 1,000 pages. No further changes were attempted.';return null}}
      // Re-read metadata immediately before mutation; scans may be older than the review dialog.
      let before=a.action==='restore'?{{status:200,body:await findInTrash()}}:await req(base+'/node');
      if(before.status!==200||before.body?.id!==f.id)return {{status:'failed',message:trashProblem||'Could not verify the selected file before changing it.'}};
      const n=before.body;
      const changed=({preflight})(f,n);
      if(changed)return {{status:'failed',message:'The file changed since scanning ('+changed+'). Refresh and review it again.'}};
      if(a.action==='trash'&&isTrashed(n)||a.action==='restore'&&!isTrashed(n))return {{status:'failed',message:'File state changed since scanning. Refresh before retrying.'}};
      // Send exactly one mutation. A timeout can occur after the service commits it.
      let mutation=null;
      try{{if(a.action==='trash'){{const q=new URLSearchParams({{soft_delete:'true',file_name:f.name}});if(f.file_id)q.set('file_id',f.file_id);if(f.parent)q.set('parent_directory_id',f.parent);mutation=await req(base+'?'+q,'DELETE')}}else mutation=await req(base+'/restore','POST')}}catch{{}}
      // Verify state independently of the mutation HTTP code; successful transport is not proof.
      let after=null;try{{after=await req(base+'/node')}}catch{{}}
      if(after?.status===200&&after.body?.id===f.id&&Object.hasOwn(after.body,'trashed_at')&&(a.action==='trash'?isTrashed(after.body):!isTrashed(after.body)))return {{status:'verified',message:a.action==='trash'?'Moved to Trash and verified.':'Restored and verified.',node:after.body}};
      // Retry only readback for eventual consistency, never the DELETE/restore request.
      if(a.action==='trash'){{for(let attempt=0;attempt<3;attempt++){{if(attempt)await new Promise(resolve=>setTimeout(resolve,750));try{{const match=await findInTrash();if(match&&isTrashed(match))return {{status:'verified',message:'Moved to Trash and verified.',node:match}}}}catch{{}}}}}}
      if(mutation&&mutation.status>=400&&after?.status===200&&after.body?.id===f.id&&(a.action==='trash'?!isTrashed(after.body):isTrashed(after.body)))return {{status:'failed',message:'Service rejected the change (HTTP '+mutation.status+'). File state is unchanged.'}};
      // Preserve ambiguity so the coordinator blocks this file instead of risking a duplicate action.
      return {{status:'uncertain',message:'The request was sent, but its result could not be verified'+(mutation?' (HTTP '+mutation.status+')':'')+'. The file may already be in Trash. '+(trashProblem||'')+' The batch stopped; refresh Library and Trash before trying again.'}};
    }})()"#
    );
    service::evaluate_json(view, script, move |r| {
        let _ = proxy.send_event(AppEvent::ChangeResult(generation, r));
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
    fn queued_batches_run_fifo_only_after_the_active_batch_finishes() {
        let mut job = Job {
            action: Action::Trash,
            remaining: vec![row("libfile_a")].into(),
            current: None,
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
        job.current.take();
        assert_eq!(next_batch(Some(&job), &mut waiting).unwrap().nonce, 20);
        assert_eq!(next_batch(None, &mut waiting).unwrap().nonce, 30);
    }
    #[test]
    fn confirmed_batches_reserve_files_without_blocking_new_selections() {
        let mut job = Job {
            action: Action::Trash,
            remaining: vec![row("libfile_a"), row("libfile_b")].into(),
            current: None,
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
        job.current.take();
        job.stop = true;
        job.take_next();
        assert!(reserved_ids(Some(&job), &VecDeque::new()).is_empty());
    }
    #[test]
    fn stopping_and_queue_limits_reject_new_confirmations() {
        let job = Job {
            action: Action::Trash,
            remaining: VecDeque::new(),
            current: Some(row("libfile_a")),
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
    fn stop_does_not_start_more_files_or_duplicate_current() {
        let f =
            parse_row(&json!({"id":"libfile_x","name":"A","file_id":"f-x","access_kind":"owned"}))
                .unwrap();
        let mut j = Job {
            action: Action::Trash,
            remaining: vec![f.clone(), f].into(),
            current: None,
            outcomes: vec![],
            stop: false,
        };
        assert!(j.take_next().is_some());
        assert!(j.take_next().is_none());
        assert_eq!(j.remaining.len(), 1);
        j.stop = true;
        j.current.take();
        assert!(j.take_next().is_none());
        assert_eq!(j.outcomes[0].status, "skipped");
    }
}
