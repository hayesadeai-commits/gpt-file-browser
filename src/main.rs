#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Application coordinator and native event loop.
//!
//! The trusted local shell owns controls and IPC; a separate WebView owns the
//! signed-in ChatGPT session. Network callbacks return typed events to this loop
//! rather than mutating UI state directly. Listing replies are matched to the
//! request that issued them, so replies to abandoned requests are inert; reviewed
//! mutation nonces bind confirmation to a specific selection.

mod capture;
mod changes;
mod download;
mod export;
mod icon;
mod inventory;
mod preview;
mod schema;
mod service;

use schema::Observation;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::Path,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tao::platform::windows::WindowExtWindows;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Theme, WindowBuilder},
};
use wry::{
    NewWindowResponse, Rect, WebContext, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

/// Handle that browser callbacks use to post events back to the main loop.
type Proxy = EventLoopProxy<AppEvent>;
/// A listing snapshot keyed by stable file ID, so the published order is deterministic.
type Rows = BTreeMap<String, inventory::FileRow>;

/// Messages crossing from shell IPC or asynchronous browser callbacks into the
/// main thread. Numeric tokens identify the request, preview epoch, or reviewed job.
enum AppEvent {
    Command(String),
    Observation(u64, Observation),
    Status(String),
    Session(service::Session),
    ListingComparison(Result<Value, String>),
    /// Request ID and the listing/usage reply.
    ApiResult(u64, Result<Value, String>),
    /// Epoch, file ID, listed revision, image, enlarged, interrupted (retryable).
    PreviewResult(u64, String, Option<String>, Option<String>, bool, bool),
    TrashResult(u64, Result<Value, String>),
    ChangeResult(u64, String, Result<Value, String>),
    ChangeSent(u64, String, service::Session, Result<Value, String>),
}

/// Listing page sizes to request, largest first; `None` is the service default.
/// Legacy native node requests omit a size, so a larger page is an optimization the
/// service is free to refuse. A page that fails at an unconfirmed size steps down one
/// size and restarts the traversal, ending at the default node-request shape. See `Scan::shrinks_for` for when a size is kept.
const PAGE_SIZES: [Option<u32>; 3] = [Some(100), Some(50), None];
/// Upper bound on overlapping listing requests. Each cursor chain stays sequential;
/// only independent folders are read at the same time.
const SCAN_CONCURRENCY: usize = 6;
/// Outstanding image loads across thumbnails and hover previews, including old epochs.
const PREVIEW_LIMIT: usize = 10;

// The only pages the app itself navigates to, and the storage-quota endpoint it reads.
const USAGE_URL: &str = "https://chatgpt.com/backend-api/files/library/storage/usage";
const SPACE_URL: &str = "https://chatgpt.com/space?tab=all";
const HOME_URL: &str = "https://chatgpt.com/";

// Status-line texts. Kept together so the wording stays consistent across handlers.
const MSG_BUSY: &str = "Wait for the current operation to finish.";
const MSG_CONNECTING: &str =
    "Connecting to your signed-in Library. If this stalls, open ChatGPT and sign in.";
const MSG_CONNECT_FAILED: &str =
    "Could not connect to Library. Open ChatGPT, sign in, then scan again.";
const MSG_SCAN_STOPPED: &str = "Scan stopped. These results are incomplete.";
const MSG_CONTEXT_CHANGED: &str = "Sign-in context changed. Scan again.";
const MSG_SMALLER_PAGES: &str =
    "A larger listing page failed. Restarting the scan with smaller pages…";
const MSG_NAV_PAUSED_JOB: &str =
    "ChatGPT navigation is paused while queued file changes are processing.";
const MSG_NAV_PAUSED_LISTING: &str =
    "ChatGPT navigation is paused while files are being listed. Stop the scan to browse.";
const MSG_CLOSING: &str = "Finishing in-flight file checks before closing…";

/// A line for the always-visible status area. Alerts are refusals and failures the
/// user needs to notice; the shell renders them more prominently than progress.
struct Note {
    text: String,
    alert: bool,
}
/// A routine progress or completion message.
fn info(text: impl Into<String>) -> Option<Note> {
    Some(Note {
        text: text.into(),
        alert: false,
    })
}
/// A refusal or failure, shown in the warning color.
fn alert(text: impl Into<String>) -> Option<Note> {
    Some(Note {
        text: text.into(),
        alert: true,
    })
}

/// Expected reply kind for one in-flight Library request.
enum Pending {
    Page(inventory::PageTask),
    Usage,
}

/// Why a listing page could not be used.
enum PageError {
    /// A different page size cannot help: a rejected sign-in or a traversal limit.
    Final(String),
    /// The service answered, but not with a usable listing page.
    Refused(String),
    /// No answer arrived: a timeout, a network failure, or the page navigated away.
    Transport(String),
}

/// One Library traversal: the snapshot being built and its outstanding requests.
#[derive(Default)]
struct Scan {
    inv: inventory::Inventory,
    running: bool,
    /// Outstanding requests by ID. IDs are never reused, so clearing this map is
    /// enough to make every earlier reply inert.
    pending: HashMap<u64, Pending>,
    requests: u64,
    usage: Option<Value>,
    started: Option<Instant>,
    /// Index into `PAGE_SIZES` for the requests being issued.
    size: usize,
    /// The size index of the last traversal that completed, once one has.
    proven: Option<usize>,
    /// Fallback is remembered for this session after the fast endpoint fails.
    legacy: bool,
}

impl Scan {
    /// Listing pages requested but not yet answered (the usage request is not counted).
    fn pages_in_flight(&self) -> usize {
        self.pending
            .values()
            .filter(|p| matches!(p, Pending::Page(_)))
            .count()
    }
    /// Start an empty traversal. Requests are issued by `pump` once a session exists.
    fn restart(&mut self) {
        self.inv = if self.legacy {
            inventory::Inventory::start()
        } else {
            inventory::Inventory::start_fast()
        };
        self.pending.clear();
        self.running = true;
    }
    /// Stop issuing and accepting pages. The partial snapshot stays visible but incomplete.
    fn halt(&mut self, error: impl Into<String>) {
        self.pending.clear();
        self.running = false;
        self.inv.complete = false;
        self.inv.error = Some(error.into());
    }
    /// Fill the free request slots. The scan finishes when the queue is empty and
    /// every issued page has been accepted.
    fn pump(
        &mut self,
        browser: &wry::WebView,
        session: &service::Session,
        proxy: &Proxy,
    ) -> Result<(), String> {
        let mut flying = self.pages_in_flight();
        while flying < SCAN_CONCURRENCY {
            let Some(task) = self.inv.next()? else { break };
            self.requests += 1;
            service::list_page(
                browser,
                session,
                &task,
                PAGE_SIZES[self.size],
                self.requests,
                proxy.clone(),
            )?;
            self.pending.insert(self.requests, Pending::Page(task));
            flying += 1;
        }
        if self.inv.complete {
            self.running = false;
            self.proven = Some(self.size);
        }
        Ok(())
    }
    /// Quota figures are informational: they load beside the listing and never gate it.
    fn request_usage(&mut self, browser: &wry::WebView, session: &service::Session, proxy: &Proxy) {
        self.requests += 1;
        if service::get(
            browser,
            session,
            USAGE_URL,
            false,
            self.requests,
            proxy.clone(),
        )
        .is_ok()
        {
            self.pending.insert(self.requests, Pending::Usage);
        }
    }
    /// Issue the first requests of a (re)started traversal.
    fn begin(
        &mut self,
        browser: &wry::WebView,
        session: &service::Session,
        proxy: &Proxy,
    ) -> Result<(), String> {
        self.request_usage(browser, session, proxy);
        self.pump(browser, session, proxy)
    }
    /// Validate and merge one listing reply, then issue the work it unlocked.
    fn advance(
        &mut self,
        task: &inventory::PageTask,
        result: Result<Value, String>,
        browser: &wry::WebView,
        session: Option<&service::Session>,
        proxy: &Proxy,
    ) -> Result<(), PageError> {
        let body = page_body(result)?;
        self.inv.accept(task, &body).map_err(PageError::Refused)?;
        let session = session.ok_or_else(|| PageError::Final("Session unavailable".into()))?;
        self.pump(browser, session, proxy).map_err(PageError::Final)
    }
    /// Fall back once, discarding partial fast results; authentication failures are final.
    fn falls_back_for(&self, error: &PageError) -> bool {
        !self.legacy && !matches!(error, PageError::Final(_))
    }
    /// Whether a failed page is worth retrying with smaller pages.
    /// A refusal always is. A missing answer is only blamed on the page size while that
    /// size is unconfirmed; once a traversal has completed with it, timeouts and network
    /// failures are ordinary failures and the faster size is kept.
    fn shrinks_for(&self, error: &PageError) -> bool {
        self.size + 1 < PAGE_SIZES.len()
            && match error {
                PageError::Final(_) => false,
                PageError::Refused(_) => true,
                PageError::Transport(_) => self.proven != Some(self.size),
            }
    }
    /// Move to the next smaller page size; the size being left is no longer trusted.
    fn shrink(&mut self) {
        if self.proven == Some(self.size) {
            self.proven = None;
        }
        self.size = (self.size + 1).min(PAGE_SIZES.len() - 1);
    }
    /// Report a failed traversal. If even the fallback sizes failed, page size was not the
    /// cause, so the next scan starts again from the best size known to work.
    fn fail(&mut self, error: impl Into<String>) {
        self.halt(error);
        self.size = self.proven.unwrap_or(0);
    }
    /// Files in the snapshot, excluding folders.
    fn file_count(&self) -> usize {
        self.inv.rows.values().filter(|r| !r.folder).count()
    }
}

/// Classify one listing reply and take out its JSON body.
fn page_body(result: Result<Value, String>) -> Result<Value, PageError> {
    let mut response = result.map_err(PageError::Transport)?;
    match response.get("status").and_then(Value::as_u64).unwrap_or(0) {
        200 => {}
        code @ (401 | 403) => {
            return Err(PageError::Final(format!(
                "ChatGPT rejected the saved sign-in (HTTP {code}). \
                 Open Library in the ChatGPT view, then scan again."
            )));
        }
        code => {
            return Err(PageError::Refused(format!(
                "Library returned HTTP {code}. Refresh ChatGPT and try again."
            )));
        }
    }
    match response.get_mut("body").map(Value::take) {
        Some(body) if !body.is_null() => Ok(body),
        _ => Err(PageError::Refused("Missing response body".into())),
    }
}

/// The separately listed Trash snapshot. Its generation rejects replies issued
/// before a reload or a sign-in change.
#[derive(Default)]
struct Trash {
    rows: Rows,
    busy: bool,
    complete: bool,
    error: Option<String>,
    generation: u64,
    cursors: HashSet<String>,
}

impl Trash {
    /// Invalidate both visible Trash data and callbacks issued under the old context.
    /// Incrementing the generation is essential even if the old request is still running.
    fn invalidate(&mut self) {
        self.generation += 1;
        self.busy = false;
        self.complete = false;
        self.rows.clear();
        self.cursors.clear();
        self.error = Some("Sign-in context changed. Reload Trash.".into());
    }
}

/// Quarantine only the affected file after a failed/uncertain action.
/// Unrelated rows remain eligible without forcing another complete scan.
fn block_file_change(inv: &mut inventory::Inventory, trash: &mut Rows, id: &str) {
    if let Some(row) = inv.rows.get_mut(id) {
        row.action_blocked = true;
    }
    if let Some(row) = trash.get_mut(id) {
        row.action_blocked = true;
    }
}

/// Invoke one shell callback with JSON data. Only display metadata crosses this
/// boundary; session headers never do.
fn call(shell: &wry::WebView, function: &str, data: &Value) {
    let _ = shell.evaluate_script(&format!("window.{function}({data})"));
}

/// Listing flags sent with every Files update: running, complete, error, pages read.
fn files_state(scan: &Scan) -> Value {
    json!({"busy": scan.running, "complete": scan.inv.complete, "error": scan.inv.error, "pages": scan.inv.pages})
}
/// Listing flags sent with every Trash update.
fn trash_state(trash: &Trash) -> Value {
    json!({"busy": trash.busy, "complete": trash.complete, "error": trash.error})
}

/// Replace a view's rows with the authoritative native snapshot. Rows are written
/// straight to text; a large collection is never copied into an intermediate tree.
fn publish_view(shell: &wry::WebView, function: &str, rows: &Rows, state: Value) {
    let rows =
        serde_json::to_string(&rows.values().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into());
    let _ = shell.evaluate_script(&format!(
        "window.{function}(Object.assign({state},{{rows:{rows}}}))"
    ));
}
/// Send the complete Files snapshot and its listing flags to the shell.
fn publish_inventory(shell: &wry::WebView, scan: &Scan) {
    publish_view(shell, "showInventory", &scan.inv.rows, files_state(scan));
}
/// Send the complete Trash snapshot and its listing flags to the shell.
fn publish_trash(shell: &wry::WebView, trash: &Trash) {
    publish_view(shell, "showTrash", &trash.rows, trash_state(trash));
}
/// Send only what changed. Listing pages and per-file results arrive far more often
/// than full snapshots, so their cost must not grow with the size of the collection.
/// `state` carries the listing flags when they may have changed; row-only updates omit it.
fn patch_view(
    shell: &wry::WebView,
    view: &str,
    upsert: &[&inventory::FileRow],
    remove: &[&str],
    state: Option<Value>,
) {
    let state = state.unwrap_or_else(|| json!({}));
    let upsert = serde_json::to_string(upsert).unwrap_or_else(|_| "[]".into());
    let (view, remove) = (json!(view), json!(remove));
    let _ = shell.evaluate_script(&format!(
        "window.patchView(Object.assign({state},{{view:{view},upsert:{upsert},remove:{remove}}}))"
    ));
}

/// Publish progress and the outcomes not yet sent, without mutation credentials.
/// `sent` tracks how many outcomes of this job the shell already holds.
fn publish_job(shell: &wry::WebView, job: &changes::Job, id: u64, sent: &mut usize) {
    let count = |status: &str| job.outcomes.iter().filter(|o| o.status == status).count();
    let (verified, skipped) = (count("verified"), count("skipped"));
    let offset = (*sent).min(job.outcomes.len());
    let data = json!({
        "id": id,
        "action": job.action,
        "active": !job.current.is_empty(),
        "stopping": job.stop,
        "remaining": job.remaining.len() + job.current.len(),
        "verified": verified,
        "requests_sent": job.requests_sent,
        "awaiting_confirmation": job.confirmations.len(),
        "sending": job.current.len().saturating_sub(job.confirmations.len()),
        "confirmation_limit": changes::UNCONFIRMED_LIMIT,
        "confirmation_backpressure": !job.stop && !job.remaining.is_empty() && job.current.len()>=changes::UNCONFIRMED_LIMIT,
        "confirmation_rows": job.confirmations.iter().filter_map(|(id,c)| job.current.get(id).map(|f|json!({"id":id,"name":f.name,"verifying":c.verifying}))).collect::<Vec<_>>(),
        "skipped": skipped,
        "attention": job.outcomes.len() - verified - skipped,
        "offset": offset,
        "outcomes": &job.outcomes[offset..],
    });
    *sent = job.outcomes.len();
    call(shell, "showJob", &data);
}
/// Hide/unselect only confirmed reservations. Inventory remains authoritative until
/// each remote operation is verified, so cancelled or failed work is recoverable.
fn publish_queue(
    shell: &wry::WebView,
    job: Option<&changes::Job>,
    waiting: &VecDeque<changes::Review>,
) {
    let data = json!({
        "ids": changes::reserved_ids(job, waiting),
        "batches": waiting.len(),
        "files": waiting.iter().map(|r| r.files.len()).sum::<usize>(),
    });
    call(shell, "showQueue", &data);
}

/// Cancel confirmed but unstarted batches, preserving a receipt for each one.
/// Nothing is replayed on restart: queued work requires a new user review.
fn cancel_waiting(waiting: &mut VecDeque<changes::Review>, root: &Path) -> Result<(), String> {
    let mut error = None;
    while let Some(r) = waiting.pop_front() {
        let mut j = changes::Job {
            action: r.action,
            remaining: r.files.into(),
            current: BTreeMap::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: Vec::new(),
            stop: true,
        };
        j.take_next();
        if let Err(e) = journal(root, &j, r.nonce) {
            error = Some(e);
        }
    }
    error.map_or(Ok(()), Err)
}

/// Mutable batch state shared by the handlers that start or advance reviewed work.
struct Batches {
    job: Option<changes::Job>,
    waiting: VecDeque<changes::Review>,
    /// Nonce of the batch whose files are in flight; independent of newer review dialogs.
    active_nonce: u64,
    /// Outcomes of the active job already published to the shell.
    sent: usize,
    /// What earlier files learned about confirming a change without reading all of Trash.
    lookup: changes::TrashLookup,
}

impl Batches {
    /// Whether a file's mutation or verification is in flight right now.
    fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !j.current.is_empty())
    }
    /// Start the next confirmed batch only after every active file finishes.
    fn start_queued(
        &mut self,
        browser: &wry::WebView,
        session: &service::Session,
        proxy: &Proxy,
        root: &Path,
    ) -> Result<(), String> {
        if let Some(r) = changes::next_batch(self.job.as_ref(), &mut self.waiting) {
            self.active_nonce = r.nonce;
            self.sent = 0;
            let mut next = changes::Job {
                action: r.action,
                remaining: r.files.into(),
                current: BTreeMap::new(),
                confirmations: BTreeMap::new(),
                requests_sent: 0,
                outcomes: Vec::new(),
                stop: false,
            };
            let result = change_next(
                browser,
                session,
                &mut next,
                r.nonce,
                self.lookup,
                proxy,
                root,
            );
            self.job = Some(next);
            if result.is_err() {
                let _ = cancel_waiting(&mut self.waiting, root);
            }
            result?;
        }
        Ok(())
    }
    /// Send the current reservations and the active job's progress to the shell.
    fn publish(&mut self, shell: &wry::WebView) {
        publish_queue(shell, self.job.as_ref(), &self.waiting);
        if let Some(j) = &self.job {
            publish_job(shell, j, self.active_nonce, &mut self.sent);
        }
    }
}

/// Record pending work and outcomes in the private LocalAppData directory.
/// Write a temporary file then rename it to avoid exposing a partially written JSON
/// receipt. Receipts contain private filenames/IDs and must not enter source control.
fn journal(root: &Path, job: &changes::Job, nonce: u64) -> Result<(), String> {
    let dir = root.join("reports");
    std::fs::create_dir_all(&dir).map_err(|_| "Could not create the action log directory")?;
    let report = job_receipt(job);
    let temp = dir.join(format!("actions-{nonce}.tmp"));
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(&report).map_err(|_| "Could not prepare action log")?,
    )
    .map_err(|_| "Could not save the action log")?;
    std::fs::rename(&temp, dir.join(format!("actions-{nonce}.json")))
        .map_err(|_| "Could not commit the action log".into())
}

/// Preserve all outstanding files even when responses finish out of order.
/// Null means no request remains in flight, including for recovery tooling.
fn job_receipt(job: &changes::Job) -> Value {
    json!({
        "action": job.action,
        "outcomes": job.outcomes,
        "pending": if job.current.is_empty() {None} else {Some(&job.current)},
        "remaining": job.remaining.len(),
        "queued_files": job.remaining,
        "requests_sent": job.requests_sent,
        "confirmations": job.confirmations.iter().map(|(id,c)|json!({"id":id,"sent":c.sent,"mutation_status":c.mutation_status,"verifying":c.verifying})).collect::<Vec<_>>(),
    })
}

/// Pump independent send/readback pools, journaling reservations before mutation.
/// The shared unfinished budget stays reserved until readback; reads never resend mutations.
fn change_next(
    browser: &wry::WebView,
    session: &service::Session,
    job: &mut changes::Job,
    nonce: u64,
    lookup: changes::TrashLookup,
    proxy: &Proxy,
    root: &Path,
) -> Result<(), String> {
    // Verification has its own capacity and remains active when sending is stopped.
    while let Some((id, receipt)) = job.next_confirmation() {
        if let Some(row) = job.current.get(&id)
            && let Err(e) = changes::verify(browser, row, &receipt, nonce, lookup, proxy.clone())
        {
            let _ = proxy.send_event(AppEvent::ChangeResult(nonce, id, Err(e)));
        }
    }
    while let Some(row) = job.take_next() {
        let result = journal(root, job, nonce).and_then(|_| {
            changes::run(
                browser,
                session,
                &row,
                job.action,
                nonce,
                lookup,
                proxy.clone(),
            )
        });
        if let Err(e) = result {
            job.current.remove(&row.id);
            job.stop = true;
            job.outcomes.push(changes::Outcome {
                id: row.id,
                name: row.name,
                status: "failed".into(),
                message: e.clone(),
            });
            job.take_next();
            return Err(e);
        }
    }
    Ok(())
}

/// Convert logical layout coordinates to WebView bounds, clamping empty dimensions.
fn bounds(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.), h.max(1.)).into(),
    }
}

/// Parse the browser-pane rectangle reported by the shell layout: `[x, y, width, height]`
/// in logical pixels. The shell is trusted, but a malformed value must not move the view.
fn pane_rect(raw: &str) -> Option<[f64; 4]> {
    let rect: [f64; 4] = serde_json::from_str(raw).ok()?;
    rect.iter()
        .all(|v| v.is_finite() && (0. ..=100_000.).contains(v))
        .then_some(rect)
}

/// The local interface: markup, stylesheet, and scripts joined into one document.
/// Everything is bundled at compile time; the shell loads no remote resources.
fn shell_html() -> String {
    include_str!("ui.html")
        .replace("/*STYLES*/", include_str!("ui.css"))
        .replace(
            "/*INVENTORY_SCRIPT*/",
            &(include_str!("inventory.js").to_owned() + include_str!("thumbnails.js")),
        )
}

/// Milliseconds since the Unix epoch; the basis for review nonces.
fn now_millis() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// Only plain service IDs are placed into a ChatGPT URL.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Launch the desktop app; smoke tests use exit codes instead of an interactive error dialog.
fn main() {
    if let Err(e) = run() {
        if std::env::args().any(|a| a == "--smoke-test") {
            std::process::exit(1);
        }
        let message = windows::core::HSTRING::from(format!(
            "GPT File Browser could not start.\n\n{e}\n\nThe Microsoft WebView2 Runtime is required."
        ));
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                &message,
                windows::core::w!("GPT File Browser"),
                windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
    }
}

/// Resolve private runtime storage without falling back to a checkout-relative path.
/// Reject missing/relative LOCALAPPDATA rather than writing a session beside the program.
fn private_data_root(
    local_app_data: Option<std::ffi::OsString>,
) -> Result<std::path::PathBuf, &'static str> {
    let base = std::path::PathBuf::from(
        local_app_data.ok_or("LOCALAPPDATA is unavailable; private storage cannot be located")?,
    );
    if !base.is_absolute() {
        return Err("LOCALAPPDATA must be an absolute directory");
    }
    Ok(base.join("GPTStorageBrowser"))
}

/// Create both WebViews and own all mutable scan, review, and batch state.
/// Browser callbacks communicate through AppEvent, keeping updates serialized.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Never derive private storage from the executable or checkout directory.
    // Running a development build must not create cookies or receipts in the repository.
    let root = private_data_root(std::env::var_os("LOCALAPPDATA"))?;
    std::fs::create_dir_all(&root)?;
    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("GPT File Browser")
        .with_window_icon(Some(tao::window::Icon::from_rgba(icon::rgba(64), 64, 64)?))
        .with_inner_size(tao::dpi::LogicalSize::new(1280., 720.))
        .with_min_inner_size(tao::dpi::LogicalSize::new(980., 680.))
        .build(&event_loop)?;
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    // Match the stylesheet's canvas color so the window does not flash white while loading.
    let canvas = if window.theme() == Theme::Dark {
        (27, 28, 30, 255)
    } else {
        (255, 255, 255, 255)
    };
    // Keep trusted local UI storage separate from ChatGPT cookies and authentication.
    let mut shell_context = WebContext::new(Some(root.join("app-data").join("interface")));
    let mut browser_context = WebContext::new(Some(root.join("app-data").join("chatgpt-profile")));
    let cp = proxy.clone();
    let shell_loaded = Cell::new(false);
    // Only bundled HTML receives an IPC bridge. Remote pages must never replace it.
    let shell = WebViewBuilder::new_with_web_context(&mut shell_context)
        .with_html(shell_html())
        .with_background_color(canvas)
        .with_ipc_handler(move |req| {
            let _ = cp.send_event(AppEvent::Command(req.body().clone()));
        })
        .with_navigation_handler(move |url| {
            // Wry converts bundled HTML to a data URL on Windows. Permit that
            // initial navigation only; external pages must never enter this IPC view.
            if !shell_loaded.get() && url.starts_with("data:text/html;") {
                shell_loaded.set(true);
                true
            } else {
                url == "about:blank"
            }
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_bounds(bounds(0., 0., size.width, size.height))
        .build_as_child(&window)?;
    let enabled = Rc::new(Cell::new(false));
    let epoch = Rc::new(Cell::new(0u64));
    let observations = Rc::new(RefCell::new(Vec::<Observation>::new()));
    let np = proxy.clone();
    let dp = proxy.clone();
    let completed_proxy = proxy.clone();
    let owner = window.hwnd();
    // This second WebView hosts normal ChatGPT pages and the authenticated fetch context.
    // The shell reports the exact pane rectangle ("bounds:") once its layout is known.
    let browser = WebViewBuilder::new_with_web_context(&mut browser_context)
        .with_bounds(bounds(208., 44., size.width - 208., size.height - 44.))
        .with_visible(false)
        .with_navigation_handler(move |url| {
            let ok = schema::allowed_navigation(&url) || url == "about:blank";
            if !ok {
                let _ = np.send_event(AppEvent::Status("External navigation blocked.".into()));
            }
            ok
        })
        .with_download_started_handler(move |_, path| {
            let accepted = download::choose_path(owner, path);
            let _ = dp.send_event(AppEvent::Status(
                if accepted {
                    "Downloading the selected file…"
                } else {
                    "Download cancelled."
                }
                .into(),
            ));
            accepted
        })
        .with_download_completed_handler(move |_, _, success| {
            let _ = completed_proxy.send_event(AppEvent::Status(
                if success {
                    "Download saved."
                } else {
                    "Download did not complete."
                }
                .into(),
            ));
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .build_as_child(&window)?;
    capture::install(&browser, enabled.clone(), epoch.clone(), proxy.clone())?;
    let mut compare_pending = std::env::args().any(|a| a == "--compare-listings");
    let mut opened = false;
    if compare_pending {
        browser.load_url(SPACE_URL)?;
        opened = true;
    }
    let mut session: Option<service::Session> = None;
    // Count actual outstanding work across UI epochs; clearing a cache does not cancel fetches.
    let mut preview_inflight = 0usize;
    let mut scan = Scan::default();
    let mut trash = Trash::default();
    let mut review: Option<changes::Review> = None;
    let mut action_nonce = 0u64;
    let mut batches = Batches {
        job: None,
        waiting: VecDeque::new(),
        active_nonce: 0,
        sent: 0,
        lookup: changes::TrashLookup::default(),
    };
    // A completed old-session action is still journaled, but must not repopulate new-session rows.
    let mut job_context_valid = true;
    let mut connection_deadline: Option<Instant> = None;
    let mut close_after_actions = false;
    let mut shell_ready = false;
    let smoke_deadline = std::env::args()
        .any(|a| a == "--smoke-test")
        .then(|| Instant::now() + Duration::from_secs(5));
    // Every callback is serialized here. The deadlines wake an otherwise idle event loop.
    event_loop.run(move |event, _, flow| {
        *flow = smoke_deadline
            .or(connection_deadline)
            .map(ControlFlow::WaitUntil)
            .unwrap_or(ControlFlow::Wait);
        if connection_deadline.is_some_and(|t| Instant::now() >= t) {
            connection_deadline = None;
            scan.halt(MSG_CONNECT_FAILED);
            publish_inventory(&shell, &scan);
        }
        if smoke_deadline.is_some_and(|t| Instant::now() >= t) {
            if !shell_ready {
                std::process::exit(2);
            }
            *flow = ControlFlow::Exit;
        }
        let mut status: Option<Note> = None;
        match event {
            // Finish every in-flight verification before closing; skip unstarted files.
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if batches.busy() {
                    if let Some(j) = batches.job.as_mut() {
                        j.stop = true;
                    }
                    let _ = cancel_waiting(&mut batches.waiting, &root);
                    close_after_actions = true;
                    batches.publish(&shell);
                    status = info(MSG_CLOSING);
                } else {
                    *flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. },
                ..
            } => {
                // The shell re-measures its browser pane and reports it after this resize.
                let sz = window.inner_size().to_logical::<f64>(window.scale_factor());
                let _ = shell.set_bounds(bounds(0., 0., sz.width, sz.height));
            }
            Event::UserEvent(AppEvent::Command(cmd)) => {
                // Commands that navigate or reload the request-host WebView.
                let navigates = ["home", "space", "refresh"].contains(&cmd.as_str())
                    || cmd.starts_with("open:")
                    || (cmd == "browser" && !opened);
                match cmd.as_str() {
                    "__ready" => shell_ready = true,
                    _ if cmd.starts_with("bounds:") => {
                        if let Some([x, y, w, h]) = pane_rect(&cmd[7..]) {
                            let _ = browser.set_bounds(bounds(x, y, w, h));
                        }
                    }
                    // Reloading/navigating the request-host WebView can discard an active
                    // mutation promise. Keep the browser hidden until its verification finishes.
                    _ if batches.busy() && (navigates || cmd == "browser") => {
                        status = alert(MSG_NAV_PAUSED_JOB);
                        let _ = browser.set_visible(false);
                        call(&shell, "showFiles", &Value::Null);
                    }
                    // The same navigation would discard in-flight listing requests and fail the
                    // scan. Viewing the already-loaded page stays available.
                    _ if navigates && (scan.pages_in_flight() > 0 || trash.busy) => {
                        status = alert(MSG_NAV_PAUSED_LISTING);
                    }
                    "browser" | "home" | "space" => {
                        let _ = browser.set_visible(true);
                        if !opened || cmd != "browser" {
                            let url = if cmd == "space" { SPACE_URL } else { HOME_URL };
                            if browser.load_url(url).is_ok() {
                                opened = true;
                            }
                        }
                    }
                    "inventory" | "evidence" => {
                        let _ = browser.set_visible(false);
                    }
                    // Abandon any earlier traversal; its outstanding replies become inert.
                    "scan" => {
                        if trash.busy || batches.busy() {
                            status = alert(MSG_BUSY);
                        } else {
                            call(&shell, "clearPreviews", &Value::Null);
                            review = None;
                            connection_deadline = None;
                            scan.restart();
                            scan.usage = None;
                            scan.started = Some(Instant::now());
                            let _ = browser.set_visible(false);
                            // Requests run inside the signed-in page. If there is no session yet,
                            // or the view was left on another site, load Library and wait for it.
                            let on_chatgpt = browser
                                .url()
                                .is_ok_and(|u| u.starts_with("https://chatgpt.com/"));
                            match &session {
                                Some(s) if on_chatgpt => {
                                    if let Err(e) = scan.begin(&browser, s, &proxy) {
                                        scan.halt(e);
                                    }
                                }
                                _ => {
                                    let _ = browser.load_url(SPACE_URL);
                                    opened = true;
                                    connection_deadline =
                                        Some(Instant::now() + Duration::from_secs(40));
                                    status = info(MSG_CONNECTING);
                                }
                            }
                            publish_inventory(&shell, &scan);
                        }
                    }
                    "cancel" => {
                        if scan.running {
                            connection_deadline = None;
                            scan.halt(MSG_SCAN_STOPPED);
                            publish_inventory(&shell, &scan);
                        }
                    }
                    // Trash maintains its own generation, cursor history, and completeness flag.
                    "load_trash" => {
                        if scan.running || batches.busy() {
                            status = alert(MSG_BUSY);
                        } else if let Some(s) = &session {
                            review = None;
                            trash.generation += 1;
                            trash.rows.clear();
                            trash.cursors.clear();
                            trash.complete = false;
                            trash.error = None;
                            trash.busy = true;
                            if let Err(e) = service::list_trash(
                                &browser,
                                s,
                                None,
                                trash.generation,
                                proxy.clone(),
                            ) {
                                trash.error = Some(e);
                                trash.busy = false;
                            }
                            publish_trash(&shell, &trash);
                        } else {
                            status = alert("Scan your Library first to establish the connection.");
                        }
                    }
                    "stop_actions" => {
                        let _ = cancel_waiting(&mut batches.waiting, &root);
                        if let Some(j) = batches.job.as_mut() {
                            j.stop = true;
                        }
                        batches.publish(&shell);
                    }
                    "cancel_review" => review = None,
                    // Resolve IDs against native snapshots; never trust row metadata sent by the
                    // shell. Selection limits, ownership, completeness, and reservations are
                    // checked before review.
                    _ if cmd.starts_with("review:") => {
                        let parsed = (|| -> Result<changes::Review, String> {
                            if scan.running || trash.busy {
                                return Err("Wait for the current scan to finish.".into());
                            }
                            if session.is_none() {
                                return Err("Connect to ChatGPT first.".into());
                            }
                            let input: Value =
                                serde_json::from_str(&cmd[7..]).map_err(|_| "Invalid selection")?;
                            let action: changes::Action = serde_json::from_value(
                                input.get("action").cloned().ok_or("Missing action")?,
                            )
                            .map_err(|_| "Invalid action")?;
                            let (source, listed) = match action {
                                changes::Action::Trash => (&scan.inv.rows, scan.inv.complete),
                                changes::Action::Restore => (&trash.rows, trash.complete),
                            };
                            if !listed {
                                return Err("Complete a fresh scan before changing files.".into());
                            }
                            let ids = input
                                .get("ids")
                                .and_then(Value::as_array)
                                .ok_or("Missing selection")?;
                            if ids.is_empty() || ids.len() > 5000 {
                                return Err("Select between 1 and 5,000 files.".into());
                            }
                            let reserved =
                                changes::reserved_ids(batches.job.as_ref(), &batches.waiting);
                            let mut seen = HashSet::new();
                            let mut files = Vec::new();
                            for id in ids {
                                let id = id.as_str().ok_or("Invalid file ID")?;
                                if !seen.insert(id) {
                                    continue;
                                }
                                if reserved.contains(id) {
                                    return Err("This file is already in a batch.".into());
                                }
                                let f = source
                                    .get(id)
                                    .ok_or("Selection is stale. Refresh the list.")?;
                                if f.action_blocked {
                                    return Err(format!(
                                        "{} needs a refresh to verify its previous action before retrying.",
                                        f.name
                                    ));
                                }
                                if !changes::eligible(f) {
                                    return Err(format!(
                                        "{} must be managed in ChatGPT (not an owned ordinary file).",
                                        f.name
                                    ));
                                }
                                files.push(f.clone());
                            }
                            action_nonce = now_millis()
                                .unwrap_or(action_nonce + 1)
                                .max(action_nonce + 1);
                            Ok(changes::Review {
                                nonce: action_nonce,
                                action,
                                files,
                            })
                        })();
                        match parsed {
                            Ok(r) => {
                                call(&shell, "showReview", &json!(r));
                                review = Some(r);
                            }
                            Err(e) => status = alert(e),
                        }
                    }
                    // Consume only the currently displayed review nonce, preventing
                    // replay/double-submit.
                    _ if cmd.starts_with("confirm:") => {
                        if let Ok(n) = cmd[8..].parse::<u64>()
                            && review.as_ref().is_some_and(|r| r.nonce == n)
                            && let Some(r) = review.take()
                        {
                            let accepted = (|| -> Result<(), String> {
                                changes::validate_queue(&r, batches.job.as_ref(), &batches.waiting)?;
                                let s = session.as_ref().ok_or("Connect to ChatGPT first.")?;
                                let queued = changes::Job {
                                    action: r.action,
                                    remaining: r.files.clone().into(),
                                    current: BTreeMap::new(),
                confirmations: BTreeMap::new(), requests_sent:0,
                                    outcomes: Vec::new(),
                                    stop: false,
                                };
                                journal(&root, &queued, r.nonce)?;
                                batches.waiting.push_back(r);
                                job_context_valid = true;
                                batches.start_queued(&browser, s, &proxy, &root)
                            })();
                            if let Err(e) = accepted {
                                status = alert(e);
                            }
                            batches.publish(&shell);
                        }
                    }
                    // Exports are private local artifacts, never part of the source package.
                    "export_csv" => {
                        let result = (|| -> Result<_, Box<dyn std::error::Error>> {
                            let dir = root.join("reports");
                            std::fs::create_dir_all(&dir)?;
                            let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
                            let path = dir.join(format!("inventory-{stamp}.csv"));
                            let quote = export::csv_cell;
                            let mut csv = String::from("Name,Bytes,Modified,Type,Parent,ID\r\n");
                            for f in scan.inv.rows.values().filter(|f| !f.folder) {
                                csv.push_str(&format!(
                                    "{},{},{},{},{},{}\r\n",
                                    quote(&f.name),
                                    f.bytes.map(|b| b.to_string()).unwrap_or_default(),
                                    quote(f.modified.as_deref().unwrap_or("")),
                                    quote(&f.mime),
                                    quote(&f.parent),
                                    quote(&f.id)
                                ));
                            }
                            std::fs::write(&path, csv)?;
                            let summary = json!({
                                "complete": scan.inv.complete,
                                "pages": scan.inv.pages,
                                "files": scan.file_count(),
                                "usage": scan.usage,
                            });
                            std::fs::write(
                                dir.join(format!("inventory-summary-{stamp}.json")),
                                serde_json::to_vec_pretty(&summary)?,
                            )?;
                            Ok(path)
                        })();
                        status = match result {
                            Ok(p) => info(format!("Inventory saved: {}", p.display())),
                            Err(e) => alert(format!("Export failed: {e}")),
                        };
                    }
                    "refresh" => {
                        if opened {
                            let _ = browser.reload();
                        }
                    }
                    "capture:on" => {
                        enabled.set(true);
                        status = info(
                            "Inspection on. Open Library, browse a folder, and load more items.",
                        );
                    }
                    "capture:off" => {
                        enabled.set(false);
                        epoch.set(epoch.get() + 1);
                        status = info("Inspection paused.");
                    }
                    "clear" => {
                        epoch.set(epoch.get() + 1);
                        observations.borrow_mut().clear();
                        call(&shell, "showEvidence", &json!([]));
                    }
                    "export" => {
                        let data = json!({
                            "build": "rust-access-check-1",
                            "completeInventoryVerified": false,
                            "observations": *observations.borrow(),
                        });
                        let result = (|| -> Result<_, Box<dyn std::error::Error>> {
                            let dir = root.join("reports");
                            std::fs::create_dir_all(&dir)?;
                            let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
                            let path = dir.join(format!("schema-{stamp}.json"));
                            std::fs::write(&path, serde_json::to_vec_pretty(&data)?)?;
                            Ok(path)
                        })();
                        status = match result {
                            Ok(p) => info(format!("Report saved: {}", p.display())),
                            Err(e) => alert(format!("Report could not be saved: {e}")),
                        };
                    }
                    // Validate ID/revision against native rows. Saturation is retryable, not a
                    // bad image.
                    _ if cmd.starts_with("thumbnail:") => {
                        if let Ok(input) = serde_json::from_str::<Value>(&cmd[10..])
                            && let (Some(id), Some(epoch)) =
                                (input["id"].as_str(), input["epoch"].as_u64())
                        {
                            let full = input["full"].as_bool().unwrap_or(false);
                            let modified = input["modified"].as_str().map(str::to_owned);
                            let row = scan.inv.rows.get(id).or_else(|| trash.rows.get(id));
                            let saturated = preview_inflight >= PREVIEW_LIMIT;
                            let mut started = false;
                            if !saturated
                                && let (Some(f), Some(s)) = (row, &session)
                                && f.modified == modified
                                && preview::supported(f)
                            {
                                started =
                                    preview::load(&browser, s, f, epoch, full, proxy.clone())
                                        .is_ok();
                            }
                            if started {
                                preview_inflight += 1;
                            } else {
                                let data = json!({
                                    "epoch": epoch,
                                    "id": id,
                                    "modified": modified,
                                    "data": null,
                                    "full": full,
                                    "retry": saturated,
                                });
                                call(&shell, "showPreview", &data);
                            }
                        }
                    }
                    _ if cmd.starts_with("open:") => {
                        if let Some(f) = scan.inv.rows.get(&cmd[5..]).filter(|f| plain_id(&f.id)) {
                            let kind = if f.folder { "d" } else { "file" };
                            let url = format!("https://chatgpt.com/space/{kind}/{}", f.id);
                            if browser.load_url(&url).is_ok() {
                                opened = true;
                            }
                            let _ = browser.set_visible(true);
                            call(&shell, "selectBrowser", &Value::Null);
                        }
                    }
                    _ => {}
                }
            }
            // A changed credential/account context invalidates reviews and both listings.
            // Do not merge an old-account callback into the newly authenticated account.
            Event::UserEvent(AppEvent::Session(s)) => {
                connection_deadline = None;
                // A scan that has not listed anything yet is only waiting for this sign-in.
                let connecting =
                    scan.running && scan.pages_in_flight() == 0 && scan.inv.pages == 0;
                if session.as_ref().is_some_and(|old| {
                    old.authorization != s.authorization || old.account != s.account
                }) {
                    call(&shell, "clearPreviews", &Value::Null);
                    review = None;
                    let _ = cancel_waiting(&mut batches.waiting, &root);
                    job_context_valid = false;
                    if let Some(j) = batches.job.as_mut() {
                        j.stop = true;
                    }
                    batches.lookup = changes::TrashLookup::default();
                    scan.halt(MSG_CONTEXT_CHANGED);
                    scan.inv = inventory::Inventory::default();
                    scan.inv.error = Some(MSG_CONTEXT_CHANGED.into());
                    scan.usage = None;
                    if connecting {
                        // Nothing from the previous context was listed, so the waiting scan
                        // simply proceeds under the new one.
                        scan.restart();
                    }
                    trash.invalidate();
                    call(&shell, "invalidateContext", &Value::Null);
                    call(&shell, "showUsage", &Value::Null);
                    publish_queue(&shell, batches.job.as_ref(), &batches.waiting);
                    publish_inventory(&shell, &scan);
                    publish_trash(&shell, &trash);
                }
                // A scan that was waiting for sign-in starts as soon as the session is known.
                if scan.running
                    && scan.pages_in_flight() == 0
                    && let Err(e) = scan.begin(&browser, &s, &proxy)
                {
                    scan.halt(e);
                    publish_inventory(&shell, &scan);
                }
                if compare_pending {
                    compare_pending=false;
                    if let Err(e)=service::compare_listings(&browser,&s,proxy.clone()) {status=alert(e);}
                    else {status=info("Comparing listing endpoints (read-only). Please leave this window open.");}
                }
                session = Some(s);
            }
            // Only requests still registered may update the snapshot being built.
            Event::UserEvent(AppEvent::ApiResult(request, result)) => {
                match scan.pending.remove(&request) {
                    // Optional quota figures: a failure here never invalidates the listing.
                    Some(Pending::Usage) => {
                        let body = result.ok().filter(|r| r["status"] == 200).map(|mut r| r["body"].take());
                        if let Some(body) = body.filter(Value::is_object) {
                            call(&shell, "showUsage", &body);
                            scan.usage = Some(body);
                        }
                    }
                    Some(Pending::Page(task)) if scan.running => {
                        let outcome =
                            scan.advance(&task, result, &browser, session.as_ref(), &proxy);
                        let fast_fallback=outcome.as_ref().err().is_some_and(|e|scan.falls_back_for(e));
                        let restarted = match outcome {
                            Ok(()) => false,
                            // A failed fast file/folder listing never becomes a partial "complete"
                            // inventory. Clear all its outstanding requests and restart legacy traversal.
                            Err(error) if scan.falls_back_for(&error) => {
                                scan.legacy=true;scan.size=0;scan.proven=None;scan.restart();
                                let begun=match &session {Some(s)=>scan.begin(&browser,s,&proxy),None=>Err("Session unavailable".into())};
                                if let Err(e)=begun {scan.fail(e);}
                                true
                            }
                            // A larger page may be what the service refused or timed out on:
                            // start over one size smaller before reporting a failure.
                            Err(error) if scan.shrinks_for(&error) => {
                                scan.shrink();
                                scan.restart();
                                let begun = match &session {
                                    Some(s) => scan.begin(&browser, s, &proxy),
                                    None => Err("Session unavailable".into()),
                                };
                                if let Err(e) = begun {
                                    scan.fail(e);
                                }
                                true
                            }
                            Err(
                                PageError::Final(e)
                                | PageError::Refused(e)
                                | PageError::Transport(e),
                            ) => {
                                scan.fail(e);
                                false
                            }
                        };
                        if scan.running && !restarted {
                            let state = files_state(&scan);
                            patch_view(&shell, "files", &scan.inv.take_fresh(), &[], Some(state));
                        } else {
                            publish_inventory(&shell, &scan);
                        }
                        status = if restarted && scan.running {
                            info(if fast_fallback {"Fast listing unavailable. Restarting with compatible folder traversal…"} else {MSG_SMALLER_PAGES})
                        } else if scan.running {
                            info(format!(
                                "Scanning… {} entries across {} pages.",
                                scan.inv.rows.len(),
                                scan.inv.pages
                            ))
                        } else if scan.inv.complete {
                            info(format!(
                                "Scan complete: {} files in {} pages ({:.1} s).",
                                scan.file_count(),
                                scan.inv.pages,
                                scan.started.map_or(0., |t| t.elapsed().as_secs_f64())
                            ))
                        } else {
                            alert(scan.inv.error.clone().unwrap_or("Scan stopped".into()))
                        };
                    }
                    _ => {}
                }
            }
            Event::UserEvent(AppEvent::Observation(generation, item)) => {
                if enabled.get()
                    && generation == epoch.get()
                    && observations.borrow().len() < schema::LIMIT
                {
                    observations.borrow_mut().push(item);
                    call(&shell, "showEvidence", &json!(*observations.borrow()));
                    status = info(format!(
                        "{} response structures observed. Complete inventory is not verified.",
                        observations.borrow().len()
                    ));
                }
            }
            Event::UserEvent(AppEvent::Status(s)) => status = info(s),
            // Require an explicit terminal cursor; a missing field is not an empty final page.
            Event::UserEvent(AppEvent::TrashResult(g, result)) => {
                if g == trash.generation && trash.busy {
                    let mut added = Vec::new();
                    let page = (|| -> Result<(), String> {
                        let value = result?;
                        if value["status"] != 200 {
                            return Err(format!("Trash returned HTTP {}.", value["status"]));
                        }
                        let body = &value["body"];
                        let cursor = inventory::page_cursor(body)?;
                        let items = body["items"].as_array().ok_or("Trash response changed")?;
                        for item in items {
                            if item["id"]
                                .as_str()
                                .is_some_and(|id| id.starts_with("libfile_"))
                            {
                                let row = inventory::parse_row(item)?;
                                added.push(row.id.clone());
                                trash.rows.insert(row.id.clone(), row);
                            }
                        }
                        match cursor {
                            None => {
                                trash.busy = false;
                                trash.complete = true;
                            }
                            Some(c) => {
                                if !trash.cursors.insert(c.clone()) || trash.cursors.len() >= 1000 {
                                    return Err(
                                        "Trash repeated a cursor or exceeded 1,000 pages.".into()
                                    );
                                }
                                service::list_trash(
                                    &browser,
                                    session.as_ref().ok_or("Session unavailable")?,
                                    Some(&c),
                                    g,
                                    proxy.clone(),
                                )?;
                            }
                        }
                        Ok(())
                    })();
                    if let Err(e) = page {
                        trash.busy = false;
                        trash.complete = false;
                        trash.error = Some(e);
                    }
                    if trash.busy {
                        let rows: Vec<_> =
                            added.iter().filter_map(|id| trash.rows.get(id)).collect();
                        patch_view(&shell, "trash", &rows, &[], Some(trash_state(&trash)));
                    } else {
                        publish_trash(&shell, &trash);
                    }
                }
            }
            Event::UserEvent(AppEvent::ChangeSent(n,id,original_session,result))=>{
                if n==batches.active_nonce && let Some(j)=batches.job.as_mut() && j.current.contains_key(&id) && !j.confirmations.contains_key(&id) {
                    let value=result.unwrap_or(Value::Null);
                    if let Some(sent)=value["sent"].as_u64() {
                        let http=value["mutation_status"].as_u64().and_then(|v|u16::try_from(v).ok());
                        j.request_sent(&id,changes::Confirmation{session:original_session,sent,mutation_status:http,verifying:false});
                        // An ambiguous/rejected response still requires readback, but does not
                        // justify sending more mutations while that ambiguity is resolved.
                        if http.is_none_or(|code|!(200..300).contains(&code)){j.stop=true;}
                        if let Err(e)=journal(&root,j,n){j.stop=true;status=alert(e);}
                        if let Some(s)=&session && let Err(e)=change_next(&browser,s,j,n,batches.lookup,&proxy,&root){status=alert(e);}
                        publish_job(&shell,j,n,&mut batches.sent);
                    }else{
                        j.stop=true;let _=proxy.send_event(AppEvent::ChangeResult(n,id,Err("Request result missing confirmation context; refresh before retrying.".into())));
                    }
                }
            }
            // Apply only verified metadata. Other outcomes stop this batch and block that file,
            // leaving unrelated rows eligible for a later review without another full scan.
            Event::UserEvent(AppEvent::ChangeResult(n, id, result)) => {
                if n == batches.active_nonce
                    && let Some(j) = batches.job.as_mut()
                    && let Some(f) = j.current.remove(&id)
                {
                    j.confirmations.remove(&id);
                    let value =
                        result.unwrap_or_else(|e| json!({"status": "uncertain", "message": e}));
                    let state = value["status"].as_str().unwrap_or("uncertain").to_string();
                    let message = value["message"]
                        .as_str()
                        .unwrap_or("Unrecognized action result")
                        .to_string();
                    let trashing = j.action == changes::Action::Trash;
                    // Hints about cheaper confirmation reads apply to the files that follow.
                    if job_context_valid {batches.lookup.learn(&value["lookup"]);}
                    if state == "verified" && job_context_valid {
                        let row = inventory::parse_row(&value["node"]).unwrap_or_else(|_| f.clone());
                        let (from, to) = if trashing { ("files", "trash") } else { ("trash", "files") };
                        patch_view(&shell, to, &[&row], &[], None);
                        if trashing {
                            scan.inv.rows.remove(&f.id);
                            trash.rows.insert(f.id.clone(), row);
                        } else {
                            trash.rows.remove(&f.id);
                            scan.inv.rows.insert(f.id.clone(), row);
                        }
                        patch_view(&shell, from, &[], &[&f.id], None);
                    } else {
                        j.stop = true;
                        if job_context_valid {
                            block_file_change(&mut scan.inv, &mut trash.rows, &f.id);
                            for (view, rows) in [("files", &scan.inv.rows), ("trash", &trash.rows)] {
                                if let Some(row) = rows.get(&f.id) {
                                    patch_view(&shell, view, &[row], &[], None);
                                }
                            }
                        }
                    }
                    j.outcomes.push(changes::Outcome {
                        id: f.id,
                        name: f.name,
                        status: state,
                        message,
                    });
                    // Persist this result even if another file is still verifying. Journal
                    // failure stops new work but must not abandon already-started checks.
                    if let Err(e)=journal(&root,j,n) {j.stop=true;status=alert(e);}
                    if let Some(s) = &session
                        && let Err(e) =
                            change_next(&browser, s, j, n, batches.lookup, &proxy, &root)
                    {
                        status = alert(e);
                    }
                    if status.is_none() {
                        status = info(format!(
                            "{} batch: {} verified, {} remaining.",
                            if trashing { "Trash" } else { "Restore" },
                            j.outcomes.iter().filter(|o| o.status == "verified").count(),
                            j.remaining.len() + j.current.len()
                        ));
                    }
                    let finished = j.current.is_empty();
                    let stopped = j.stop;
                    // Persist the final drained/skipped state once all in-flight checks finish.
                    if finished && let Err(e) = journal(&root, j, n) {
                        status = alert(e);
                    }
                    // Show this batch's final result before a queued batch replaces it.
                    publish_job(&shell, j, n, &mut batches.sent);
                    if finished && stopped {
                        let cancelled = batches.waiting.len();
                        if let Err(e) = cancel_waiting(&mut batches.waiting, &root) {
                            status = alert(e);
                        } else if cancelled > 0 {
                            status = info(format!(
                                "Queue stopped; {cancelled} waiting batches cancelled. Unprocessed files are available again."
                            ));
                        }
                    }
                    if close_after_actions && finished {
                        *flow = ControlFlow::Exit;
                    } else if finished
                        && !stopped
                        && let Some(s) = &session
                        && let Err(e) = batches.start_queued(&browser, s, &proxy, &root)
                    {
                        status = alert(e);
                    }
                    // Each outcome releases its own reservation in the shell; resend the full
                    // reservation list only when the set of batches changed.
                    if finished {
                        batches.publish(&shell);
                    }
                }
            }
            Event::UserEvent(AppEvent::ListingComparison(result))=>{
                let report=result.unwrap_or_else(|e|json!({"error":e}));
                let dir=root.join("reports");let _=std::fs::create_dir_all(&dir);
                if std::fs::write(dir.join("listing-comparison.json"),serde_json::to_vec_pretty(&report).unwrap_or_default()).is_ok(){status=info("Read-only listing comparison completed.");}
                else{status=alert("Could not save listing comparison summary.");}
            }
            // Release native capacity even for stale UI epochs; the shell rejects their images.
            Event::UserEvent(AppEvent::PreviewResult(epoch, id, modified, image, full, retry)) => {
                preview_inflight = preview_inflight.saturating_sub(1);
                let data = json!({
                    "epoch": epoch,
                    "id": id,
                    "modified": modified,
                    "data": image,
                    "full": full,
                    "retry": retry,
                });
                call(&shell, "showPreview", &data);
            }
            _ => {}
        }
        if let Some(note) = status {
            let _ = shell.evaluate_script(&format!(
                "window.showStatus({},{})",
                json!(note.text),
                note.alert
            ));
        }
    });
}

#[cfg(test)]
mod state_tests {
    use super::*;
    #[test]
    fn receipts_preserve_each_outstanding_file() {
        let row = |id| inventory::parse_row(&json!({"id":id,"name":"fixture.txt"})).unwrap();
        let mut job = changes::Job {
            action: changes::Action::Trash,
            current: BTreeMap::from([
                ("libfile_a".into(), row("libfile_a")),
                ("libfile_b".into(), row("libfile_b")),
            ]),
            remaining: VecDeque::new(),
            confirmations: BTreeMap::new(),
            requests_sent: 0,
            outcomes: vec![],
            stop: false,
        };
        assert_eq!(job_receipt(&job)["pending"].as_object().unwrap().len(), 2);
        job.request_sent(
            "libfile_a",
            changes::Confirmation {
                session: service::Session {
                    url: "fixture-url-secret".into(),
                    authorization: "fixture-token-secret".into(),
                    account: Some("fixture-account-secret".into()),
                },
                sent: 123,
                mutation_status: Some(200),
                verifying: false,
            },
        );
        let serialized = job_receipt(&job).to_string();
        assert!(!serialized.contains("secret"));
        assert_eq!(job_receipt(&job)["confirmations"][0]["sent"], 123);
        job.current.remove("libfile_b");
        assert!(job_receipt(&job)["pending"].get("libfile_a").is_some());
        job.current.clear();
        assert!(job_receipt(&job)["pending"].is_null());
    }
    #[test]
    fn private_storage_never_falls_back_to_the_project() {
        assert!(private_data_root(None).is_err());
        assert!(private_data_root(Some("".into())).is_err());
        assert!(private_data_root(Some("relative/path".into())).is_err());
        let base = std::path::PathBuf::from(r"C:\LocalAppDataFixture");
        assert_eq!(
            private_data_root(Some(base.clone().into_os_string())).unwrap(),
            base.join("GPTStorageBrowser")
        );
    }

    #[test]
    fn failed_file_does_not_require_rescanning_unaffected_files() {
        let mut inv = inventory::Inventory::default();
        inv.complete = true;
        let mut trash = Rows::new();
        for id in ["libfile_failed", "libfile_next"] {
            let row = inventory::parse_row(
                &json!({"id":id,"name":"file.jpg","file_id":"file_source","access_kind":"owned"}),
            )
            .unwrap();
            inv.rows.insert(id.into(), row);
        }
        block_file_change(&mut inv, &mut trash, "libfile_failed");
        assert!(inv.complete);
        assert!(!changes::eligible(&inv.rows["libfile_failed"]));
        assert!(changes::eligible(&inv.rows["libfile_next"]));
    }

    #[test]
    fn account_change_cancels_pending_trash_and_discards_old_rows() {
        let row = inventory::parse_row(&json!({"id":"libfile_old","name":"old.png"})).unwrap();
        let mut trash = Trash {
            rows: Rows::from([(row.id.clone(), row)]),
            busy: true,
            complete: true,
            error: None,
            generation: 7,
            cursors: HashSet::from(["next".to_string()]),
        };
        let old_response_generation = trash.generation;
        trash.invalidate();
        assert_ne!(old_response_generation, trash.generation);
        assert!(!trash.busy && !trash.complete);
        assert!(trash.rows.is_empty() && trash.cursors.is_empty());
        assert!(trash.error.is_some());
        // An old callback remains invalid even if a new Trash request is in progress.
        trash.busy = true;
        assert!(!(old_response_generation == trash.generation && trash.busy));
    }

    #[test]
    fn stopping_a_scan_makes_outstanding_replies_inert() {
        let mut scan = Scan::default();
        scan.restart();
        let task = scan.inv.next().unwrap().unwrap();
        scan.pending.insert(1, Pending::Page(task));
        scan.pending.insert(2, Pending::Usage);
        assert_eq!(scan.pages_in_flight(), 1);
        scan.halt(MSG_SCAN_STOPPED);
        assert!(!scan.running && !scan.inv.complete);
        assert!(scan.pending.remove(&1).is_none());
        assert_eq!(scan.inv.error.as_deref(), Some(MSG_SCAN_STOPPED));
        // A new traversal starts clean and never reuses an earlier request ID.
        scan.restart();
        assert!(scan.running && scan.inv.error.is_none() && scan.pending.is_empty());
    }

    #[test]
    fn fast_listing_falls_back_once_without_keeping_partial_data() {
        let mut scan = Scan::default();
        scan.restart();
        let task = scan.inv.next().unwrap().unwrap();
        assert_eq!(task.scope, inventory::ListingScope::Files);
        scan.pending.insert(1, Pending::Page(task));
        assert!(scan.falls_back_for(&PageError::Refused("bad shape".into())));
        assert!(scan.falls_back_for(&PageError::Transport("timeout".into())));
        assert!(!scan.falls_back_for(&PageError::Final("unauthorized".into())));
        scan.legacy = true;
        scan.restart();
        assert!(scan.pending.is_empty());
        assert!(!scan.inv.complete);
        assert_eq!(
            scan.inv.next().unwrap().unwrap().scope,
            inventory::ListingScope::Nodes
        );
        assert!(!scan.falls_back_for(&PageError::Refused("bad shape".into())));
    }
    #[test]
    fn page_size_falls_back_safely_and_keeps_what_works() {
        let refused = PageError::Refused(String::new());
        let silent = PageError::Transport(String::new());
        let final_error = PageError::Final(String::new());
        let mut scan = Scan::default();
        assert_eq!(PAGE_SIZES[scan.size], Some(100));
        assert_eq!(
            *PAGE_SIZES.last().unwrap(),
            None,
            "the last resort is the native request"
        );
        // Unconfirmed: any failure that a smaller page might cure steps down.
        assert!(scan.shrinks_for(&refused) && scan.shrinks_for(&silent));
        assert!(!scan.shrinks_for(&final_error));
        scan.shrink();
        assert_eq!(PAGE_SIZES[scan.size], Some(50));
        scan.shrink();
        assert_eq!(PAGE_SIZES[scan.size], None);
        assert!(!scan.shrinks_for(&refused) && !scan.shrinks_for(&silent));
        // The service default failing too means size was never the problem (for example,
        // no network): the next scan starts from the fastest size again.
        scan.fail("offline");
        assert_eq!(PAGE_SIZES[scan.size], Some(100));
        assert_eq!(scan.inv.error.as_deref(), Some("offline"));

        // A completed traversal confirms its size. Transient failures then leave it alone...
        scan.size = 1;
        scan.proven = Some(1);
        assert!(!scan.shrinks_for(&silent));
        scan.fail("timeout");
        assert_eq!(PAGE_SIZES[scan.size], Some(50));
        // ...but an explicit refusal still falls back, and the old size is no longer trusted.
        assert!(scan.shrinks_for(&refused));
        scan.shrink();
        assert_eq!((scan.size, scan.proven), (2, None));
    }

    #[test]
    fn listing_replies_are_classified_before_they_touch_the_inventory() {
        let reply = |v: Value| page_body(Ok(v));
        for code in [401, 403] {
            assert!(matches!(
                reply(json!({"status":code,"body":{"detail":"x"}})),
                Err(PageError::Final(_))
            ));
        }
        // An answer that is not a usable page may be a refusal of the requested size.
        for bad in [
            json!({"status":422,"body":{"detail":"limit"}}),
            json!({"status":500,"body":null}),
            json!({"status":200,"body":null}),
            json!({"status":200}),
            json!({}),
        ] {
            assert!(matches!(reply(bad), Err(PageError::Refused(_))));
        }
        assert!(matches!(
            page_body(Err("timeout".into())),
            Err(PageError::Transport(_))
        ));
        let body = reply(json!({"status":200,"body":{"items":[],"cursor":null}}));
        assert!(matches!(body, Ok(b) if b["items"].is_array() && b["cursor"].is_null()));
    }

    #[test]
    fn browser_pane_rectangles_are_validated() {
        assert_eq!(
            pane_rect("[208,44,1072,676]"),
            Some([208., 44., 1072., 676.])
        );
        for bad in [
            "",
            "[1,2,3]",
            "[1,2,3,-4]",
            "[1,2,3,1e9]",
            "[null,2,3,4]",
            "{}",
        ] {
            assert_eq!(pane_rect(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_plain_ids_are_opened_in_chatgpt() {
        assert!(plain_id("libfile_abc-123"));
        for bad in ["", "libfile_../x", "libfile a", "x?y=1", "a/b"] {
            assert!(!plain_id(bad), "{bad}");
        }
    }

    #[test]
    fn shell_document_is_fully_assembled_and_local_only() {
        let html = shell_html();
        assert!(!html.contains("/*STYLES*/") && !html.contains("/*INVENTORY_SCRIPT*/"));
        assert!(html.contains("default-src 'none'"));
        for callback in [
            "showInventory",
            "showTrash",
            "patchView",
            "showQueue",
            "showJob",
            "showReview",
            "showPreview",
            "showStatus",
            "showUsage",
            "showFiles",
            "selectBrowser",
            "clearPreviews",
            "invalidateContext",
            "showEvidence",
        ] {
            assert!(
                html.contains(&format!("window.{callback} = ")),
                "{callback}"
            );
        }
        for remote in ["<script src", "<link ", "@import", "url(http", "src=\"http"] {
            assert!(!html.contains(remote), "shell must not load {remote}");
        }
    }
}
