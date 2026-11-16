//! Authenticated service calls inside the existing ChatGPT WebView.
//!
//! The in-process DevTools interface awaits browser promises and returns JSON.
//! It does not expose a remote debugging port. Authorization remains in memory;
//! callers must supply only app-generated scripts and validated service URLs.

use crate::{AppEvent, inventory::PageTask};
use tao::event_loop::EventLoopProxy;
use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
use windows::core::{HSTRING, w};
use wry::WebViewExtWindows;

// Session material stays in memory and is only sent back to chatgpt.com.
#[derive(Clone)]
/// Captured request context. Do not derive Debug/Serialize or write it to reports.
pub struct Session {
    pub url: String,
    pub authorization: String,
    pub account: Option<String>,
}

impl Session {
    /// Rebuild a native listing URL for a complete scan.
    /// Remove search/type/provider and paging filters inherited from the browser UI,
    /// then append this task's parent and cursor using URL encoding.
    pub fn page_url(&self, task: &PageTask) -> Result<String, String> {
        let mut u = url::Url::parse(&self.url).map_err(|_| "Invalid service URL")?;
        if u.scheme() != "https"
            || u.host_str() != Some("chatgpt.com")
            || u.path() != "/backend-api/files/library/nodes"
        {
            return Err("Unexpected service endpoint".into());
        }
        let pairs: Vec<_> = u
            .query_pairs()
            .filter(|(k, _)| {
                ![
                    "cursor",
                    "parent_directory_id",
                    "include_hidden_files",
                    "include_files",
                    "include_provider_roots",
                    "include_onedrive",
                    "q",
                    "source",
                    "categories",
                    "categories[]",
                ]
                .contains(&k.as_ref())
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        u.set_query(None);
        {
            let mut q = u.query_pairs_mut();
            for (k, v) in pairs {
                q.append_pair(&k, &v);
            }
            q.append_pair("include_hidden_files", "true");
            q.append_pair("include_files", "true");
            q.append_pair("include_provider_roots", "false");
            q.append_pair("include_onedrive", "false");
            if let Some(p) = &task.parent {
                q.append_pair("parent_directory_id", p);
            }
            if let Some(c) = &task.cursor {
                q.append_pair("cursor", c);
            }
        }
        Ok(u.into())
    }
}

/// Fetch only approved listing/usage endpoints and post a generation-tagged result.
/// The browser request times out and rejects oversized text before JSON parsing.
pub fn get(
    view: &wry::WebView,
    session: &Session,
    url: &str,
    generation: u64,
    proxy: EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    if !view
        .url()
        .map_err(|_| "Browser unavailable")?
        .starts_with("https://chatgpt.com/")
    {
        return Err("Open ChatGPT and sign in before scanning.".into());
    }
    let u = url::Url::parse(url).map_err(|_| "Invalid URL")?;
    if u.origin().ascii_serialization() != "https://chatgpt.com"
        || ![
            "/backend-api/files/library/nodes",
            "/backend-api/files/library/storage/usage",
        ]
        .contains(&u.path())
    {
        return Err("Request outside the Library scope".into());
    }
    let args = serde_json::json!({"url":url,"authorization":session.authorization,"account":session.account});
    let script = format!(
        r#"(async()=>{{if(location.origin!=='https://chatgpt.com')throw new Error('Sign-in required');const a={args};const c=new AbortController();const t=setTimeout(()=>c.abort(),30000);try{{const headers={{Authorization:a.authorization}};if(a.account)headers['ChatGPT-Account-ID']=a.account;const r=await fetch(a.url,{{method:'GET',credentials:'include',headers,signal:c.signal}});const text=await r.text();if(text.length>2097152)throw new Error('Listing response too large');return {{status:r.status,body:JSON.parse(text)}}}}finally{{clearTimeout(t)}}}})()"#
    );
    let params = serde_json::json!({"expression":script,"awaitPromise":true,"returnByValue":true});
    let callback =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, output| {
            let parsed = (|| {
                result.map_err(|_| "Browser request failed".to_string())?;
                let value: serde_json::Value =
                    serde_json::from_str(&output).map_err(|_| "Invalid browser result")?;
                if value.get("exceptionDetails").is_some() {
                    return Err("Request failed or timed out. Refresh ChatGPT and retry.".into());
                }
                value
                    .pointer("/result/value")
                    .cloned()
                    .ok_or_else(|| "Missing service response".into())
            })();
            let _ = proxy.send_event(AppEvent::ApiResult(generation, parsed));
            Ok(())
        }));
    // SAFETY: the WebView COM interface is used on its owning UI thread; the generated
    // completion handler owns its captured callback state for asynchronous completion.
    unsafe {
        view.webview().CallDevToolsProtocolMethod(
            w!("Runtime.evaluate"),
            &HSTRING::from(params.to_string()),
            &callback,
        )
    }
    .map_err(|_| "Could not start request".into())
}

/// Evaluate an app-owned async script and invoke done with its JSON return value.
/// Protocol failures and JavaScript exceptions become errors, never successful data.
pub fn evaluate_json(
    view: &wry::WebView,
    script: String,
    done: impl FnOnce(Result<serde_json::Value, String>) + 'static,
) -> Result<(), String> {
    let params = serde_json::json!({"expression":script,"awaitPromise":true,"returnByValue":true});
    let callback =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, output| {
            let value = (|| {
                result.map_err(|_| "Browser connection failed".to_string())?;
                let v: serde_json::Value =
                    serde_json::from_str(&output).map_err(|_| "Invalid browser response")?;
                if v.get("exceptionDetails").is_some() {
                    return Err("Request failed or timed out; refresh before retrying.".into());
                }
                v.pointer("/result/value")
                    .cloned()
                    .ok_or_else(|| "Missing service result".into())
            })();
            done(value);
            Ok(())
        }));
    // SAFETY: the WebView COM interface is used on its owning UI thread; the generated
    // completion handler owns its captured callback state for asynchronous completion.
    unsafe {
        view.webview().CallDevToolsProtocolMethod(
            w!("Runtime.evaluate"),
            &HSTRING::from(params.to_string()),
            &callback,
        )
    }
    .map_err(|_| "Could not start browser request".into())
}

/// Read one 50-item Trash page. POST is the service's read-only listing method;
/// this function does not restore or delete anything.
pub fn list_trash(
    view: &wry::WebView,
    session: &Session,
    cursor: Option<&str>,
    generation: u64,
    proxy: EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    let args = serde_json::json!({"authorization":session.authorization,"account":session.account,"cursor":cursor});
    let script = format!(
        r#"(async()=>{{if(location.origin!=='https://chatgpt.com')throw Error('Sign in required');const a={args};const headers={{Authorization:a.authorization,'Content-Type':'application/json'}};if(a.account)headers['ChatGPT-Account-ID']=a.account;const r=await fetch('/backend-api/files/library',{{method:'POST',credentials:'include',headers,body:JSON.stringify({{limit:50,cursor:a.cursor,trashed_only:true,include_hidden_files:true}}),signal:AbortSignal.timeout(30000)}});return {{status:r.status,body:await r.json()}}}})()"#
    );
    evaluate_json(view, script, move |r| {
        let _ = proxy.send_event(AppEvent::TrashResult(generation, r));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_scan_cannot_inherit_native_search_or_type_filters() {
        let s=Session{url:"https://chatgpt.com/backend-api/files/library/nodes?entry_surface=spaces&q=secret&categories=image&source=uploaded&include_files=false&include_hidden_files=false".into(),authorization:"not-exported".into(),account:None};
        let url = s
            .page_url(&PageTask {
                parent: None,
                cursor: Some("a&b=c".into()),
            })
            .unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert!(!q.contains_key("q"));
        assert!(!q.contains_key("categories"));
        assert!(!q.contains_key("source"));
        assert_eq!(q["include_files"], "true");
        assert_eq!(q["include_hidden_files"], "true");
        assert_eq!(q["cursor"], "a&b=c");
        assert!(!url.contains("not-exported"));
    }
    #[test]
    fn endpoint_context_is_not_reusable_on_another_host() {
        let s = Session {
            url: "https://evil.test/backend-api/files/library/nodes".into(),
            authorization: "private".into(),
            account: None,
        };
        assert!(
            s.page_url(&PageTask {
                parent: None,
                cursor: None
            })
            .is_err()
        );
    }
}
