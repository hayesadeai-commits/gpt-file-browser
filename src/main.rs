#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Application coordinator and native event loop.
//!
//! The trusted local shell owns controls and IPC; a separate WebView owns the
//! signed-in ChatGPT session. Network callbacks return typed events to this loop
//! rather than mutating UI state directly. Scan/Trash generations reject stale
//! callbacks; reviewed mutation nonces bind confirmation to a specific selection.

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
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use tao::platform::windows::WindowExtWindows;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{
    NewWindowResponse, Rect, WebContext, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

/// Messages crossing from shell IPC or asynchronous browser callbacks into the
/// main thread. Numeric tokens identify the scan, preview epoch, or reviewed job.
enum AppEvent {
    Command(String),
    Observation(u64, Observation),
    Status(String),
    Session(service::Session),
    ApiResult(u64, Result<serde_json::Value, String>),
    PreviewResult(u64, String, Option<String>, Option<String>, bool),
    TrashResult(u64, Result<serde_json::Value, String>),
    ChangeResult(u64, Result<serde_json::Value, String>),
}

/// Quarantine only the affected file after a failed/uncertain action.
/// Unrelated rows remain eligible without forcing another complete scan.
fn block_file_change(
    inv: &mut inventory::Inventory,
    trash: &mut std::collections::BTreeMap<String, inventory::FileRow>,
    id: &str,
) {
    if let Some(row) = inv.rows.get_mut(id) {
        row.action_blocked = true;
    }
    if let Some(row) = trash.get_mut(id) {
        row.action_blocked = true;
    }
}

/// Invalidate both visible Trash data and callbacks issued under the old context.
/// Incrementing the generation is essential even if the old request is still running.
fn invalidate_trash(
    generation: &mut u64,
    busy: &mut bool,
    complete: &mut bool,
    rows: &mut std::collections::BTreeMap<String, inventory::FileRow>,
    cursors: &mut std::collections::HashSet<String>,
    error: &mut Option<String>,
) {
    *generation += 1;
    *busy = false;
    *complete = false;
    rows.clear();
    cursors.clear();
    *error = Some("Sign-in context changed. Reload Trash.".into());
}

/// Send the normalized Trash snapshot to the shell; no session headers are included.
fn publish_trash(
    shell: &wry::WebView,
    rows: &std::collections::BTreeMap<String, inventory::FileRow>,
    busy: bool,
    complete: bool,
    error: &Option<String>,
) {
    let data = serde_json::json!({"rows":rows.values().collect::<Vec<_>>(),"busy":busy,"complete":complete,"error":error});
    let _ = shell.evaluate_script(&format!("window.showTrash({data})"));
}
/// Publish progress and per-file outcomes without exposing mutation credentials.
fn publish_job(shell: &wry::WebView, job: &changes::Job) {
    let data = serde_json::json!({"action":job.action,"outcomes":job.outcomes,"remaining":job.remaining.len(),"active":job.current.is_some(),"stopping":job.stop});
    let _ = shell.evaluate_script(&format!("window.showJob({data})"));
}
/// Hide/unselect only confirmed reservations. Inventory remains authoritative until
/// each remote operation is verified, so cancelled or failed work is recoverable.
fn publish_queue(
    shell: &wry::WebView,
    job: Option<&changes::Job>,
    waiting: &std::collections::VecDeque<changes::Review>,
) {
    let data = serde_json::json!({"ids":changes::reserved_ids(job,waiting),"batches":waiting.len(),"files":waiting.iter().map(|r|r.files.len()).sum::<usize>()});
    let _ = shell.evaluate_script(&format!("window.showQueue({data})"));
}

/// Cancel confirmed but unstarted batches, preserving a receipt for each one.
/// Nothing is replayed on restart: queued work requires a new user review.
fn cancel_waiting(
    waiting: &mut std::collections::VecDeque<changes::Review>,
    root: &std::path::Path,
) -> Result<(), String> {
    let mut error = None;
    while let Some(r) = waiting.pop_front() {
        let mut j = changes::Job {
            action: r.action,
            remaining: r.files.into(),
            current: None,
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

/// Start the next confirmed batch only after the previous active file finishes.
/// The active nonce is independent from the nonce of a newly opened review dialog.
fn start_queued(
    browser: &wry::WebView,
    session: &service::Session,
    waiting: &mut std::collections::VecDeque<changes::Review>,
    job: &mut Option<changes::Job>,
    active_nonce: &mut u64,
    proxy: tao::event_loop::EventLoopProxy<AppEvent>,
    root: &std::path::Path,
) -> Result<(), String> {
    if let Some(r) = changes::next_batch(job.as_ref(), waiting) {
        *active_nonce = r.nonce;
        let mut next = changes::Job {
            action: r.action,
            remaining: r.files.into(),
            current: None,
            outcomes: Vec::new(),
            stop: false,
        };
        let result = change_next(browser, session, &mut next, r.nonce, proxy, root);
        *job = Some(next);
        if result.is_err() {
            let _ = cancel_waiting(waiting, root);
        }
        result?;
    }
    Ok(())
}

/// Record pending work and outcomes in the private LocalAppData directory.
/// Write a temporary file then rename it to avoid exposing a partially written JSON
/// receipt. Receipts contain private filenames/IDs and must not enter source control.
fn journal(root: &std::path::Path, job: &changes::Job, nonce: u64) -> Result<(), String> {
    let dir = root.join("reports");
    std::fs::create_dir_all(&dir).map_err(|_| "Could not create the action log directory")?;
    let report = serde_json::json!({"action":job.action,"outcomes":job.outcomes,"pending":job.current,"remaining":job.remaining.len(),"queued_files":job.remaining});
    let temp = dir.join(format!("actions-{nonce}.tmp"));
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(&report).map_err(|_| "Could not prepare action log")?,
    )
    .map_err(|_| "Could not save the action log")?;
    std::fs::rename(&temp, dir.join(format!("actions-{nonce}.json")))
        .map_err(|_| "Could not commit the action log".into())
}
/// Claim one file, persist its pending receipt, then begin its preflight/mutation.
/// Failure to journal prevents the request from starting. No second file is started
/// while Job.current is occupied.
fn change_next(
    browser: &wry::WebView,
    session: &service::Session,
    job: &mut changes::Job,
    nonce: u64,
    proxy: tao::event_loop::EventLoopProxy<AppEvent>,
    root: &std::path::Path,
) -> Result<(), String> {
    if let Some(row) = job.take_next() {
        let result = journal(root, job, nonce)
            .and_then(|_| changes::run(browser, session, &row, job.action, nonce, proxy));
        if let Err(e) = result {
            job.current = None;
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

/// Expected response kind for the single in-flight Library scan request.
enum Pending {
    Page(inventory::PageTask),
    Usage,
}

/// Schedule the next listing task, or fetch usage after traversal completes.
/// The generation travels with the request so cancelled scans cannot overwrite newer data.
fn advance(
    browser: &wry::WebView,
    session: &service::Session,
    inv: &mut inventory::Inventory,
    pending: &mut Option<Pending>,
    generation: u64,
    proxy: tao::event_loop::EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    if let Some(task) = inv.next()? {
        let url = session.page_url(&task)?;
        *pending = Some(Pending::Page(task));
        service::get(browser, session, &url, generation, proxy)
    } else {
        *pending = Some(Pending::Usage);
        service::get(
            browser,
            session,
            "https://chatgpt.com/backend-api/files/library/storage/usage",
            generation,
            proxy,
        )
    }
}

/// Serialize display metadata and scan status into the trusted shell callback.
fn publish_inventory(
    shell: &wry::WebView,
    inv: &inventory::Inventory,
    busy: bool,
    usage: &Option<serde_json::Value>,
) {
    let data = serde_json::json!({"rows":inv.rows.values().collect::<Vec<_>>(),"complete":inv.complete,"error":inv.error,"pages":inv.pages,"busy":busy,"usage":usage});
    let _ = shell.evaluate_script(&format!("window.showInventory({data})"));
}

/// Convert logical layout coordinates to WebView bounds, clamping empty dimensions.
fn bounds(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.), h.max(1.)).into(),
    }
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
    // Keep trusted local UI storage separate from ChatGPT cookies and authentication.
    let mut shell_context = WebContext::new(Some(root.join("app-data").join("interface")));
    let mut browser_context = WebContext::new(Some(root.join("app-data").join("chatgpt-profile")));
    let cp = proxy.clone();
    let shell_loaded = Cell::new(false);
    // Only bundled HTML receives an IPC bridge. Remote pages must never replace it.
    let shell = WebViewBuilder::new_with_web_context(&mut shell_context)
        .with_html(include_str!("ui.html").replace(
            "/*INVENTORY_SCRIPT*/",
            &(include_str!("inventory.js").to_owned() + include_str!("thumbnails.js")),
        ))
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
        .with_bounds(bounds(0., 0., 1320., 860.))
        .build_as_child(&window)?;
    let enabled = Rc::new(Cell::new(false));
    let epoch = Rc::new(Cell::new(0u64));
    let observations = Rc::new(RefCell::new(Vec::<Observation>::new()));
    let np = proxy.clone();
    let dp = proxy.clone();
    let completed_proxy = proxy.clone();
    let owner = window.hwnd();
    // This second WebView hosts normal ChatGPT pages and the authenticated fetch context.
    let browser = WebViewBuilder::new_with_web_context(&mut browser_context)
        .with_bounds(bounds(196., 52., 1072., 656.))
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
    let mut opened = false;
    let mut session: Option<service::Session> = None;
    // Count actual outstanding work across UI epochs; clearing a cache does not cancel fetches.
    let mut preview_inflight = 0usize;
    let mut inv = inventory::Inventory::default();
    let mut scanning = false;
    let mut scan_generation = 0u64;
    let mut pending: Option<Pending> = None;
    let mut usage: Option<serde_json::Value> = None;
    let mut trash_rows = std::collections::BTreeMap::<String, inventory::FileRow>::new();
    let mut trash_busy = false;
    let mut trash_complete = false;
    let mut trash_error = None;
    let mut trash_generation = 0u64;
    let mut trash_cursors = std::collections::HashSet::<String>::new();
    let mut review: Option<changes::Review> = None;
    let mut action_nonce = 0u64;
    let mut active_nonce = 0u64;
    let mut waiting = std::collections::VecDeque::<changes::Review>::new();
    let mut job: Option<changes::Job> = None;
    // A completed old-session action is still journaled, but must not repopulate new-session rows.
    let mut job_context_valid = true;
    let mut connection_deadline: Option<std::time::Instant> = None;
    let mut close_after_actions = false;
    let mut shell_ready = false;
    let mut smoke_deadline = None;
    if std::env::args().any(|a| a == "--smoke-test") {
        smoke_deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    }
    // Every callback is serialized here. The deadlines wake an otherwise idle event loop.
    event_loop.run(move |event, _, flow| {
        *flow = smoke_deadline.or(connection_deadline).map(ControlFlow::WaitUntil).unwrap_or(ControlFlow::Wait);
        if connection_deadline.is_some_and(|t|std::time::Instant::now()>=t){connection_deadline=None;scanning=false;inv.error=Some("Could not connect to Library. Open ChatGPT, sign in, then scan again.".into());publish_inventory(&shell,&inv,false,&usage);}
        if smoke_deadline.is_some_and(|t| std::time::Instant::now() >= t) {
            if !shell_ready { std::process::exit(2); }
            *flow = ControlFlow::Exit;
        }
        let mut status = None;
        match event {
            // Finish verification of an active mutation before closing; skip remaining files.
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {if let Some(j)=job.as_mut().filter(|j|j.current.is_some()){j.stop=true;let _=cancel_waiting(&mut waiting,&root);publish_queue(&shell,Some(j),&waiting);close_after_actions=true;publish_job(&shell,j);status=Some("Finishing the current file check before closing…".into());}else{*flow=ControlFlow::Exit;}},
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } | Event::WindowEvent { event: WindowEvent::ScaleFactorChanged { .. }, .. } => {
                let sz = window.inner_size().to_logical::<f64>(window.scale_factor());
                let _ = shell.set_bounds(bounds(0., 0., sz.width, sz.height));
                let _ = browser.set_bounds(bounds(196., 52., sz.width - 208., sz.height - 64.));
            }
            Event::UserEvent(AppEvent::Command(cmd)) => match cmd.as_str() {
                "__ready" => shell_ready = true,
                // Reloading/navigating the request-host WebView can discard an active mutation
                // promise. Keep browsing controls local until its verification finishes.
                _ if job.as_ref().is_some_and(|j|j.current.is_some()) && (["browser","home","space","refresh"].contains(&cmd.as_str()) || cmd.starts_with("open:")) => {
                    status=Some("ChatGPT navigation is paused while queued file changes are processing.".into());
                    let _=browser.set_visible(false);let _=shell.evaluate_script("page('inventory')");
                }
                "browser" | "home" | "space" => {
                    let _ = browser.set_visible(true);
                    if !opened || cmd == "home" || cmd == "space" {
                        let url = if cmd == "space" { "https://chatgpt.com/space?tab=all" } else { "https://chatgpt.com/" };
                        if browser.load_url(url).is_ok() { opened = true; }
                    }
                }
                "overview" | "evidence" | "inventory" | "trash" => { let _ = browser.set_visible(false); }
                // Start a new generation; results from a cancelled/older scan become inert.
                "scan" => {
                    if trash_busy||job.as_ref().is_some_and(|j|j.current.is_some()){status=Some("Wait for the current operation to finish.".into());}
                    else {
                    let _=shell.evaluate_script("window.clearPreviews()");
                    review=None;
                    scan_generation+=1; inv=inventory::Inventory::start(); pending=None; usage=None; scanning=true;
                    let _=browser.set_visible(false);
                    if let Some(s)=&session {
                        if let Err(e)=advance(&browser,s,&mut inv,&mut pending,scan_generation,proxy.clone()) {inv.error=Some(e);scanning=false;}
                    } else {
                        let _=browser.load_url("https://chatgpt.com/space?tab=all"); opened=true;
                        connection_deadline=Some(std::time::Instant::now()+std::time::Duration::from_secs(40));
                        status=Some("Connecting to your signed-in Library. If this stalls, open ChatGPT and sign in.".into());
                    }
                    publish_inventory(&shell,&inv,scanning,&usage);
                    }
                }
                "cancel" => {connection_deadline=None;scan_generation+=1;pending=None;scanning=false;inv.complete=false;inv.error=Some("Scan stopped. These results are incomplete.".into());publish_inventory(&shell,&inv,scanning,&usage);}
                // Trash maintains its own generation, cursor history, and completeness flag.
                "load_trash" => {
                    if scanning||job.as_ref().is_some_and(|j|j.current.is_some()){status=Some("Wait for the current operation to finish.".into());}
                    else if let Some(s)=&session {review=None;trash_generation+=1;trash_rows.clear();trash_cursors.clear();trash_complete=false;trash_error=None;trash_busy=true;if let Err(e)=service::list_trash(&browser,s,None,trash_generation,proxy.clone()){trash_error=Some(e);trash_busy=false;}publish_trash(&shell,&trash_rows,trash_busy,trash_complete,&trash_error);}
                    else{status=Some("Scan your Library first to establish the connection.".into());}
                }
                "stop_actions"=>{let _=cancel_waiting(&mut waiting,&root);if let Some(j)=job.as_mut(){j.stop=true;publish_queue(&shell,Some(j),&waiting);publish_job(&shell,j);}},
                "cancel_review"=>review=None,
                // Resolve IDs against native snapshots; never trust row metadata sent by the shell.
                // Selection limits, ownership, completeness, and reservations are checked before review.
                _ if cmd.starts_with("review:")=>{
                    let parsed=(||->Result<changes::Review,String>{
                        if scanning||trash_busy{return Err("Wait for the current scan to finish.".into());}
                        if session.is_none(){return Err("Connect to ChatGPT first.".into());}
                        let input:serde_json::Value=serde_json::from_str(&cmd[7..]).map_err(|_|"Invalid selection")?;
                        let action:changes::Action=serde_json::from_value(input.get("action").cloned().ok_or("Missing action")?).map_err(|_|"Invalid action")?;
                        if action==changes::Action::Trash&&!inv.complete||action==changes::Action::Restore&&!trash_complete{return Err("Complete a fresh scan before changing files.".into());}
                        let ids=input.get("ids").and_then(serde_json::Value::as_array).ok_or("Missing selection")?;
                        if ids.is_empty()||ids.len()>5000{return Err("Select between 1 and 5,000 files.".into());}
                        let reserved=changes::reserved_ids(job.as_ref(),&waiting);
                        let source=if action==changes::Action::Trash{&inv.rows}else{&trash_rows};let mut seen=std::collections::HashSet::new();let mut files=Vec::new();
                        for id in ids{let id=id.as_str().ok_or("Invalid file ID")?;if !seen.insert(id){continue;}
                        if reserved.contains(id){return Err("This file is already in a batch.".into());}let f=source.get(id).ok_or("Selection is stale. Refresh the list.")?;if f.action_blocked{return Err(format!("{} needs a refresh to verify its previous action before retrying.",f.name));}
                        if !changes::eligible(f){return Err(format!("{} must be managed in ChatGPT (not an owned ordinary file).",f.name));}files.push(f.clone());}
                        action_nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as u64).unwrap_or(action_nonce+1).max(action_nonce+1);Ok(changes::Review{nonce:action_nonce,action,files})
                    })();
                    match parsed{Ok(r)=>{let data=serde_json::to_string(&r).unwrap();review=Some(r);let _=shell.evaluate_script(&format!("window.showReview({data})"));},Err(e)=>status=Some(e)}
                }
                // Consume only the currently displayed review nonce, preventing replay/double-submit.
                _ if cmd.starts_with("confirm:")=>{
                    if let Ok(n)=cmd[8..].parse::<u64>() && review.as_ref().is_some_and(|r|r.nonce==n) {
                        let r=review.take().unwrap();
                        let accepted=(||->Result<(),String>{
                            changes::validate_queue(&r,job.as_ref(),&waiting)?;
                            let s=session.as_ref().ok_or("Connect to ChatGPT first.")?;
                            let queued=changes::Job{action:r.action,remaining:r.files.clone().into(),current:None,outcomes:Vec::new(),stop:false};
                            journal(&root,&queued,r.nonce)?;
                            waiting.push_back(r);
                            job_context_valid=true;
                            start_queued(&browser,s,&mut waiting,&mut job,&mut active_nonce,proxy.clone(),&root)
                        })();
                        if let Err(e)=accepted{status=Some(e);}
                        publish_queue(&shell,job.as_ref(),&waiting);
                        if let Some(j)=&job{publish_job(&shell,j);}
                    }
                }
                // Exports are private local artifacts, never part of the source package.
                "export_csv" => {
                    let result=(|| -> Result<_,Box<dyn std::error::Error>> {
                        let dir=root.join("reports");std::fs::create_dir_all(&dir)?;
                        let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
                        let path=dir.join(format!("inventory-{stamp}.csv"));
                        let quote=export::csv_cell;
                        let mut csv=String::from("Name,Bytes,Modified,Type,Parent,ID\r\n");
                        for f in inv.rows.values().filter(|f|!f.folder) {csv.push_str(&format!("{},{},{},{},{},{}\r\n",quote(&f.name),f.bytes.map(|b|b.to_string()).unwrap_or_default(),quote(f.modified.as_deref().unwrap_or("")),quote(&f.mime),quote(&f.parent),quote(&f.id)));}
                        std::fs::write(&path,csv)?;
                        std::fs::write(dir.join(format!("inventory-summary-{stamp}.json")),serde_json::to_vec_pretty(&serde_json::json!({"complete":inv.complete,"pages":inv.pages,"files":inv.rows.values().filter(|r|!r.folder).count(),"usage":usage}))?)?;
                        Ok(path)
                    })();status=Some(match result{Ok(p)=>format!("Inventory saved: {}",p.display()),Err(e)=>format!("Export failed: {e}")});
                }
                "refresh" => { if opened { let _ = browser.reload(); } }
                "capture:on" => { enabled.set(true); status = Some("Inspection on. Open Space, browse a folder, and load more items.".into()); }
                "capture:off" => { enabled.set(false); epoch.set(epoch.get()+1); status = Some("Inspection paused.".into()); }
                "clear" => { epoch.set(epoch.get()+1); observations.borrow_mut().clear(); let _ = shell.evaluate_script("window.showEvidence([])"); }
                "export" => {
                    let data = serde_json::json!({"build":"rust-access-check-1", "completeInventoryVerified":false, "observations":*observations.borrow()});
                    let result = (|| -> Result<_, Box<dyn std::error::Error>> {
                        let dir = root.join("reports"); std::fs::create_dir_all(&dir)?;
                        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis();
                        let path = dir.join(format!("schema-{stamp}.json"));
                        std::fs::write(&path, serde_json::to_vec_pretty(&data)?)?; Ok(path)
                    })();
                    status = Some(match result { Ok(p) => format!("Report saved: {}", p.display()), Err(e) => format!("Report could not be saved: {e}") });
                }
                // Validate ID/revision against native rows. Saturation is retryable, not a bad image.
                _ if cmd.starts_with("thumbnail:")=>{
                    if let Ok(input)=serde_json::from_str::<serde_json::Value>(&cmd[10..])&& let (Some(id),Some(epoch))=(input["id"].as_str(),input["epoch"].as_u64()){
                        let full=input["full"].as_bool().unwrap_or(false);let row=inv.rows.get(id).or_else(||trash_rows.get(id));let modified=input["modified"].as_str().map(str::to_owned);
                        let mut started=false;let retry=preview_inflight>=6;
                        if let (Some(f),Some(s))=(row,&session)&& f.modified==modified&&preview::supported(f)&&preview_inflight<6&& preview::load(&browser,s,f,epoch,full,proxy.clone()).is_ok(){preview_inflight+=1;started=true;}
                        if !started{let data=serde_json::json!({"epoch":epoch,"id":id,"modified":modified,"data":null,"full":full,"retry":retry});let _=shell.evaluate_script(&format!("window.showPreview({data})"));}
                    }
                }
                _ if cmd.starts_with("open:") => {if let Some(f)=inv.rows.get(&cmd[5..]) {let url=format!("https://chatgpt.com/space/{}/{}",if f.folder{"d"}else{"file"},f.id);let _=browser.load_url(&url);let _=browser.set_visible(true);let _=shell.evaluate_script("window.selectBrowser()");}}
                _ => {}
            },
            // A changed credential/account context invalidates reviews and both listing generations.
            // Do not merge an old-account callback into the newly authenticated account.
            Event::UserEvent(AppEvent::Session(s)) => {
                connection_deadline=None;
                if session.as_ref().is_some_and(|old|old.authorization!=s.authorization||old.account!=s.account){
                    let _=shell.evaluate_script("window.clearPreviews()");
                    review=None;let _=cancel_waiting(&mut waiting,&root);job_context_valid=false;if let Some(j)=job.as_mut(){j.stop=true;}
                    scanning=false;scan_generation+=1;pending=None;inv=inventory::Inventory::default();usage=None;
                    inv.error=Some("Sign-in context changed. Scan again.".into());
                    invalidate_trash(&mut trash_generation,&mut trash_busy,&mut trash_complete,&mut trash_rows,&mut trash_cursors,&mut trash_error);
                    let _=shell.evaluate_script("window.invalidateContext()");publish_queue(&shell,job.as_ref(),&waiting);
                    publish_inventory(&shell,&inv,false,&usage);publish_trash(&shell,&trash_rows,false,false,&trash_error);
                }
                session=Some(s);
                if scanning && pending.is_none()
                    && let Err(e)=advance(&browser,session.as_ref().unwrap(),&mut inv,&mut pending,scan_generation,proxy.clone()) {inv.error=Some(e);scanning=false;publish_inventory(&shell,&inv,scanning,&usage);}
            }
            // Only the active scan may consume its pending task or publish new inventory state.
            Event::UserEvent(AppEvent::ApiResult(generation,result)) => {
                if generation==scan_generation && scanning {
                    let outcome=(||->Result<(),String>{
                        let response=result?;
                        let code=response.get("status").and_then(serde_json::Value::as_u64).unwrap_or(0);
                        if code!=200 {return Err(format!("Library returned HTTP {code}. Refresh ChatGPT and try again."));}
                        let body=response.get("body").ok_or("Missing response body")?;
                        match pending.take() {
                            Some(Pending::Page(task))=>{inv.accept(&task,body)?;advance(&browser,session.as_ref().ok_or("Session unavailable")?,&mut inv,&mut pending,scan_generation,proxy.clone())?;}
                            Some(Pending::Usage)=>{usage=Some(body.clone());scanning=false;}
                            None=>return Err("Unexpected page response".into()),
                        } Ok(())
                    })();
                    if let Err(e)=outcome {inv.error=Some(e);inv.complete=false;scanning=false;pending=None;}
                    publish_inventory(&shell,&inv,scanning,&usage);
                    status=Some(if scanning {format!("Scanning… {} entries across {} pages.",inv.rows.len(),inv.pages)} else if inv.complete {format!("Scan complete: {} files.",inv.rows.values().filter(|r|!r.folder).count())}else{inv.error.clone().unwrap_or("Scan stopped".into())});
                }
            }
            Event::UserEvent(AppEvent::Observation(generation, item)) => {
                if enabled.get() && generation == epoch.get() && observations.borrow().len() < schema::LIMIT {
                    observations.borrow_mut().push(item);
                    if let Ok(json) = serde_json::to_string(&*observations.borrow()) { let _ = shell.evaluate_script(&format!("window.showEvidence({json})")); }
                    status = Some(format!("{} response structures observed. Complete inventory is not verified.", observations.borrow().len()));
                }
            }
            Event::UserEvent(AppEvent::Status(s)) => status = Some(s),
            // Require an explicit terminal cursor; a missing field is not an empty final page.
            Event::UserEvent(AppEvent::TrashResult(g,result))=>{
                if g==trash_generation&&trash_busy {
                    let r=(||->Result<(),String>{let value=result?;if value["status"]!=200{return Err(format!("Trash returned HTTP {}.",value["status"]));}let b=&value["body"];let cursor=inventory::page_cursor(b)?;let items=b["items"].as_array().ok_or("Trash response changed")?;for item in items{if item["id"].as_str().is_some_and(|id|id.starts_with("libfile_")){let row=inventory::parse_row(item)?;trash_rows.insert(row.id.clone(),row);}}
                    match cursor{None=>{trash_busy=false;trash_complete=true;},Some(c)=>{if !trash_cursors.insert(c.clone())||trash_cursors.len()>=1000{return Err("Trash repeated a cursor or exceeded 1,000 pages.".into());}service::list_trash(&browser,session.as_ref().ok_or("Session unavailable")?,Some(&c),g,proxy.clone())?;}}Ok(())})();if let Err(e)=r{trash_busy=false;trash_complete=false;trash_error=Some(e);}publish_trash(&shell,&trash_rows,trash_busy,trash_complete,&trash_error);
                }
            }
            // Apply only verified metadata. Other outcomes stop this batch and block that file,
            // leaving unrelated rows eligible for a later review without another full scan.
            Event::UserEvent(AppEvent::ChangeResult(n,result))=>{
                if n==active_nonce&& let Some(j)=job.as_mut()&& let Some(f)=j.current.take(){
                    let value=result.unwrap_or_else(|e|serde_json::json!({"status":"uncertain","message":e}));let state=value["status"].as_str().unwrap_or("uncertain").to_string();let message=value["message"].as_str().unwrap_or("Unrecognized action result").to_string();
                    if state=="verified"&&job_context_valid{if j.action==changes::Action::Trash{inv.rows.remove(&f.id);trash_rows.insert(f.id.clone(),inventory::parse_row(&value["node"]).unwrap_or(f.clone()));}else{trash_rows.remove(&f.id);inv.rows.insert(f.id.clone(),inventory::parse_row(&value["node"]).unwrap_or(f.clone()));}}
                    else{j.stop=true;if job_context_valid{block_file_change(&mut inv,&mut trash_rows,&f.id);}}
                    j.outcomes.push(changes::Outcome{id:f.id,name:f.name,status:state,message});
                    if let Some(s)=&session&& let Err(e)=change_next(&browser,s,j,n,proxy.clone(),&root){status=Some(e);}
                    publish_inventory(&shell,&inv,scanning,&usage);publish_trash(&shell,&trash_rows,trash_busy,trash_complete,&trash_error);
                    if status.is_none(){status=Some(format!("{} batch: {} verified, {} remaining.",if j.action==changes::Action::Trash{"Trash"}else{"Restore"},j.outcomes.iter().filter(|o|o.status=="verified").count(),j.remaining.len()+usize::from(j.current.is_some())));}
                    if let Err(e)=journal(&root,j,n){status=Some(e);}
                    let finished=j.current.is_none();
                    let stopped=j.stop;
                    if finished && stopped {
                        let cancelled=waiting.len();
                        if let Err(e)=cancel_waiting(&mut waiting,&root){status=Some(e);}
                        else if cancelled>0{status=Some(format!("Queue stopped; {cancelled} waiting batches cancelled. Unprocessed files are available again."));}
                    }
                    if close_after_actions && finished {*flow=ControlFlow::Exit;}
                    else if finished && !stopped && let Some(s)=&session && let Err(e)=start_queued(&browser,s,&mut waiting,&mut job,&mut active_nonce,proxy.clone(),&root){status=Some(e);}
                    publish_queue(&shell,job.as_ref(),&waiting);
                    if let Some(j)=&job{publish_job(&shell,j);}
                }
            }
            // Release native capacity even for stale UI epochs; the shell rejects their images.
            Event::UserEvent(AppEvent::PreviewResult(epoch,id,modified,image,full))=>{preview_inflight=preview_inflight.saturating_sub(1);let data=serde_json::json!({"epoch":epoch,"id":id,"modified":modified,"data":image,"full":full});let _=shell.evaluate_script(&format!("window.showPreview({data})"));},
            _ => {}
        }
        if let Some(s) = status && let Ok(json) = serde_json::to_string(&s) { let _ = shell.evaluate_script(&format!("window.showStatus({json})")); }
    });
}

#[cfg(test)]
mod state_tests {
    use super::*;
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
        let mut trash = std::collections::BTreeMap::new();
        for id in ["libfile_failed", "libfile_next"] {
            let row=inventory::parse_row(&serde_json::json!({"id":id,"name":"file.jpg","file_id":"file_source","access_kind":"owned"})).unwrap();
            inv.rows.insert(id.into(), row);
        }
        block_file_change(&mut inv, &mut trash, "libfile_failed");
        assert!(inv.complete);
        assert!(!changes::eligible(&inv.rows["libfile_failed"]));
        assert!(changes::eligible(&inv.rows["libfile_next"]));
    }

    #[test]
    fn account_change_cancels_pending_trash_and_discards_old_rows() {
        let mut generation = 7;
        let old_response_generation = generation;
        let mut busy = true;
        let mut complete = true;
        let row = inventory::parse_row(&serde_json::json!({"id":"libfile_old","name":"old.png"}))
            .unwrap();
        let mut rows = std::collections::BTreeMap::from([(row.id.clone(), row)]);
        let mut cursors = std::collections::HashSet::from(["next".to_string()]);
        let mut error = None;
        invalidate_trash(
            &mut generation,
            &mut busy,
            &mut complete,
            &mut rows,
            &mut cursors,
            &mut error,
        );
        assert_ne!(old_response_generation, generation);
        assert!(!busy && !complete);
        assert!(rows.is_empty() && cursors.is_empty());
        assert!(error.is_some());
        // An old callback remains invalid even if a new Trash request is in progress.
        busy = true;
        assert!(!(old_response_generation == generation && busy));
    }
}
