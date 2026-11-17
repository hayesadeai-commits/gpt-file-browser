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

/// Recognize authenticated Library listing traffic independently of the UI's
/// choice of listing route. Current ChatGPT uses POST /library; older pages used
/// GET /library/nodes. Both carry the same session headers. Never learn credentials
/// from unrelated routes or hosts, and never inherit a POST search/filter body.
pub fn observed_listing_context(method: &str, raw: &str) -> Option<String> {
    let mut url = url::Url::parse(raw).ok()?;
    if url.origin().ascii_serialization() != "https://chatgpt.com" {
        return None;
    }
    match (method, url.path()) {
        ("GET", "/backend-api/files/library/nodes") => Some(url.into()),
        ("POST", "/backend-api/files/library") => {
            url.set_path("/backend-api/files/library/nodes");
            url.set_query(None);
            url.set_fragment(None);
            Some(url.into())
        }
        _ => None,
    }
}

impl Session {
    /// Rebuild a native listing URL for a complete scan.
    /// Remove search/type/provider and paging filters inherited from the browser UI,
    /// then append this task's parent and cursor using URL encoding.
    /// `limit` requests a larger page than the service default; `None` omits it.
    pub fn page_url(&self, task: &PageTask, limit: Option<u32>) -> Result<String, String> {
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
                    "limit",
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
            if let Some(n) = limit {
                q.append_pair("limit", &n.to_string());
            }
        }
        Ok(u.into())
    }
}

/// Take the awaited JavaScript value out of a `Runtime.evaluate` reply without copying it.
/// Protocol failures and JavaScript exceptions become errors, never successful data.
fn evaluated(
    result: windows::core::Result<()>,
    output: String,
    failure: &str,
) -> Result<serde_json::Value, String> {
    result.map_err(|_| "Browser connection failed".to_string())?;
    let mut reply: serde_json::Value =
        serde_json::from_str(&output).map_err(|_| "Invalid browser response")?;
    if reply.get("exceptionDetails").is_some() {
        return Err(failure.into());
    }
    reply
        .pointer_mut("/result/value")
        .map(serde_json::Value::take)
        .ok_or_else(|| "Missing service response".into())
}

/// Fetch only approved listing/usage endpoints and post a tagged result.
/// `request` identifies the individual call, so several listing pages can be
/// outstanding at once and replies to abandoned requests stay inert.
///
/// The browser script times out each attempt, rejects oversized text before JSON
/// parsing, and retries only throttling/gateway statuses (a GET listing is safe to
/// repeat). With `slim`, listing items are reduced to `ROW_FIELDS` before they cross
/// the bridge; the `items`/`cursor` shape is preserved so validation still applies.
pub fn get(
    view: &wry::WebView,
    session: &Session,
    url: &str,
    slim: bool,
    request: u64,
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
    let fields = slim.then_some(crate::inventory::ROW_FIELDS);
    let args = serde_json::json!({"url":url,"authorization":session.authorization,"account":session.account,"fields":fields});
    let script = format!(
        r#"(async()=>{{
      if(location.origin!=='https://chatgpt.com')throw new Error('Sign-in required');
      const a={args},headers={{Authorization:a.authorization}};
      if(a.account)headers['ChatGPT-Account-ID']=a.account;
      const pick=item=>{{if(item===null||typeof item!=='object')return item;const out={{}};for(const k of a.fields)if(k in item)out[k]=item[k];return out}};
      for(let attempt=0;;attempt++){{
        const c=new AbortController(),t=setTimeout(()=>c.abort(),30000);
        try{{
          const r=await fetch(a.url,{{method:'GET',credentials:'include',headers,signal:c.signal}});
          // Throttling and gateway errors are transient; back off briefly instead of failing the scan.
          if([429,502,503,504].includes(r.status)&&attempt<3){{
            const wait=Math.min(Number(r.headers.get('retry-after'))*1000||500*2**attempt,8000);
            try{{await r.body?.cancel()}}catch{{}}
            await new Promise(done=>setTimeout(done,wait));
            continue;
          }}
          const text=await r.text();
          if(text.length>2097152)throw new Error('Listing response too large');
          // A non-JSON error page still reports its HTTP status to the coordinator.
          let body=null;try{{body=JSON.parse(text)}}catch{{}}
          if(a.fields&&body&&Array.isArray(body.items))body={{items:body.items.map(pick),cursor:body.cursor}};
          return {{status:r.status,body}};
        }}finally{{clearTimeout(t)}}
      }}
    }})()"#
    );
    let params = serde_json::json!({"expression":script,"awaitPromise":true,"returnByValue":true});
    let callback =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, output| {
            let parsed = evaluated(
                result,
                output,
                "Request failed or timed out. Refresh ChatGPT and retry.",
            );
            let _ = proxy.send_event(AppEvent::ApiResult(request, parsed));
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
            done(evaluated(
                result,
                output,
                "Request failed or timed out; refresh before retrying.",
            ));
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
    fn session_discovery_supports_current_and_legacy_listing_requests() {
        assert_eq!(
            observed_listing_context(
                "POST",
                "https://chatgpt.com/backend-api/files/library?q=private"
            ),
            Some("https://chatgpt.com/backend-api/files/library/nodes".into())
        );
        assert!(
            observed_listing_context(
                "GET",
                "https://chatgpt.com/backend-api/files/library/nodes?cursor=next"
            )
            .is_some()
        );
        for url in [
            "https://evil.test/backend-api/files/library",
            "http://chatgpt.com/backend-api/files/library",
            "https://chatgpt.com:444/backend-api/files/library",
            "https://chatgpt.com/backend-api/files/library/files/example/restore",
        ] {
            assert!(observed_listing_context("POST", url).is_none());
        }
        assert!(
            observed_listing_context("DELETE", "https://chatgpt.com/backend-api/files/library")
                .is_none()
        );
    }
    #[test]
    fn full_scan_cannot_inherit_native_search_or_type_filters() {
        let s=Session{url:"https://chatgpt.com/backend-api/files/library/nodes?entry_surface=spaces&q=secret&categories=image&source=uploaded&include_files=false&include_hidden_files=false".into(),authorization:"not-exported".into(),account:None};
        let url = s
            .page_url(
                &PageTask {
                    parent: None,
                    cursor: Some("a&b=c".into()),
                },
                None,
            )
            .unwrap();
        let parsed = url::Url::parse(&url).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert!(!q.contains_key("q"));
        assert!(!q.contains_key("categories"));
        assert!(!q.contains_key("source"));
        assert_eq!(q["include_files"], "true");
        assert_eq!(q["include_hidden_files"], "true");
        assert_eq!(q["cursor"], "a&b=c");
        assert!(!q.contains_key("limit"));
        assert!(!url.contains("not-exported"));
    }
    #[test]
    fn page_size_is_app_controlled_and_never_inherited() {
        let s = Session {
            url: "https://chatgpt.com/backend-api/files/library/nodes?entry_surface=spaces&limit=5"
                .into(),
            authorization: "not-exported".into(),
            account: None,
        };
        let task = PageTask {
            parent: Some("libdir_a".into()),
            cursor: None,
        };
        let sized = url::Url::parse(&s.page_url(&task, Some(100)).unwrap()).unwrap();
        let limits: Vec<_> = sized
            .query_pairs()
            .filter(|(k, _)| k == "limit")
            .map(|(_, v)| v.into_owned())
            .collect();
        assert_eq!(limits, ["100"]);
        let default = s.page_url(&task, None).unwrap();
        assert!(!default.contains("limit="));
        assert!(default.contains("parent_directory_id=libdir_a"));
    }
    #[test]
    fn endpoint_context_is_not_reusable_on_another_host() {
        let s = Session {
            url: "https://evil.test/backend-api/files/library/nodes".into(),
            authorization: "private".into(),
            account: None,
        };
        assert!(
            s.page_url(
                &PageTask {
                    parent: None,
                    cursor: None
                },
                None
            )
            .is_err()
        );
    }
}
