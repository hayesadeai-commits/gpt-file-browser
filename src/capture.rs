//! Observe native Library requests and optionally capture sanitized schemas.
//!
//! Session discovery is independent of the user-facing inspection toggle. The
//! toggle controls diagnostic body capture only; it does not enable automation.
//! COM callbacks retain shared counters and dispatch results back to the UI loop.

use crate::{AppEvent, schema};
use std::{cell::Cell, rc::Rc};
use tao::event_loop::EventLoopProxy;
use webview2_com::{
    Microsoft::Web::WebView2::Win32::*, PermissionRequestedEventHandler,
    WebResourceResponseReceivedEventHandler, WebResourceResponseViewGetContentCompletedHandler,
    take_pwstr,
};
use windows::core::{Interface, PWSTR, w};
use wry::WebViewExtWindows;

/// Attach COM event handlers and disable password/autofill and permission prompts.
/// Observe same-service requests for session context; diagnostic body capture is
/// bounded by concurrency, response size, report count, and the current capture epoch.
pub fn install(
    view: &wry::WebView,
    enabled: Rc<Cell<bool>>,
    epoch: Rc<Cell<u64>>,
    proxy: EventLoopProxy<AppEvent>,
) -> windows::core::Result<()> {
    let core = view.webview();
    let flight = Rc::new(Cell::new(0usize));
    let count = Rc::new(Cell::new(0usize));
    let last_epoch = Rc::new(Cell::new(epoch.get()));
    // SAFETY: install runs on the WebView UI thread. COM handler wrappers retain their
    // captured Rc state; returned PWSTR allocations are consumed with take_pwstr.
    unsafe {
        let settings: ICoreWebView2Settings4 = core.Settings()?.cast()?;
        settings.SetIsPasswordAutosaveEnabled(false)?;
        settings.SetIsGeneralAutofillEnabled(false)?;
        let mut token = 0;
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let core2: ICoreWebView2_2 = core.cast()?;
        core2.add_WebResourceResponseReceived(
            &WebResourceResponseReceivedEventHandler::create(Box::new(move |_, args| {
                if last_epoch.get() != epoch.get() {
                    last_epoch.set(epoch.get());
                    count.set(0);
                }
                let Some(args) = args else {
                    return Ok(());
                };
                let request = args.Request()?;
                let mut raw = PWSTR::null();
                request.Uri(&mut raw)?;
                let url = take_pwstr(raw);
                let Some(path) = schema::candidate(&url) else {
                    return Ok(());
                };
                let query = schema::query_shape(&url);
                let mut method_ptr = PWSTR::null();
                request.Method(&mut method_ptr)?;
                let method = take_pwstr(method_ptr);
                let mut has_auth = windows::core::BOOL::default();
                request
                    .Headers()?
                    .Contains(w!("Authorization"), &mut has_auth)?;
                let authenticated_header = has_auth.as_bool();
                // Session discovery must follow both legacy and current native listing traffic.
                // Headers stay in memory; POST filters/search text are deliberately not retained.
                if let Some(listing_url) = crate::service::observed_listing_context(&method, &url)
                    && authenticated_header
                {
                    let mut auth_ptr = PWSTR::null();
                    request
                        .Headers()?
                        .GetHeader(w!("Authorization"), &mut auth_ptr)?;
                    let mut account_ptr = PWSTR::null();
                    let account = if request
                        .Headers()?
                        .GetHeader(w!("ChatGPT-Account-ID"), &mut account_ptr)
                        .is_ok()
                    {
                        Some(take_pwstr(account_ptr))
                    } else {
                        None
                    };
                    let _ = proxy.send_event(AppEvent::Session(crate::service::Session {
                        url: listing_url,
                        authorization: take_pwstr(auth_ptr),
                        account,
                    }));
                }
                // Session discovery above stays active even when diagnostic inspection is off
                // or its concurrent body reads are saturated.
                if !enabled.get() || count.get() >= schema::LIMIT || flight.get() >= 3 {
                    return Ok(());
                }
                let response = args.Response()?;
                let mut mime = PWSTR::null();
                if response
                    .Headers()?
                    .GetHeader(w!("Content-Type"), &mut mime)
                    .is_err()
                {
                    return Ok(());
                }
                if !take_pwstr(mime).to_ascii_lowercase().contains("json") {
                    return Ok(());
                }
                let mut status = 0;
                response.StatusCode(&mut status)?;
                let generation = epoch.get();
                let f = flight.clone();
                let c = count.clone();
                let p = proxy.clone();
                let on = enabled.clone();
                let gen_cell = epoch.clone();
                flight.set(flight.get() + 1);
                let callback = WebResourceResponseViewGetContentCompletedHandler::create(Box::new(
                    move |result, stream| {
                        // Always release capacity, even if capture was stopped while GetContent ran.
                        f.set(f.get().saturating_sub(1));
                        if !on.get() || gen_cell.get() != generation || c.get() >= schema::LIMIT {
                            return Ok(());
                        }
                        if result.is_err() {
                            let _ = p.send_event(AppEvent::Status(
                                "A response body was unavailable.".into(),
                            ));
                            return Ok(());
                        }
                        let Some(stream) = stream else {
                            return Ok(());
                        };
                        // Read incrementally so an oversized diagnostic body is never retained in full.
                        let mut data = Vec::new();
                        loop {
                            let mut buf = [0u8; 8192];
                            let mut got = 0;
                            let hr = stream.Read(
                                buf.as_mut_ptr().cast(),
                                buf.len() as u32,
                                Some(&mut got),
                            );
                            if hr.is_err() {
                                return Ok(());
                            }
                            if got == 0 {
                                break;
                            }
                            if data.len() + got as usize > schema::MAX_BYTES {
                                let _ = p.send_event(AppEvent::Status(
                                    "Skipped a response larger than 2 MiB.".into(),
                                ));
                                return Ok(());
                            }
                            data.extend_from_slice(&buf[..got as usize]);
                        }
                        if let Ok(structure) = schema::describe(&data) {
                            c.set(c.get() + 1);
                            let _ = p.send_event(AppEvent::Observation(
                                generation,
                                schema::Observation {
                                    path,
                                    method,
                                    query,
                                    authenticated_header,
                                    status,
                                    schema: structure,
                                },
                            ));
                        }
                        Ok(())
                    },
                ));
                if response.GetContent(&callback).is_err() {
                    flight.set(flight.get().saturating_sub(1));
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}
