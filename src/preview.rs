//! Bounded, memory-only image fetching and rasterization.
//!
//! Thumbnail loading falls back to original content on HTTP, network, or decode
//! failure. Images are decoded in the signed-in browser and returned as PNG data
//! URLs to the local shell; no image cache files are written by this module.

use crate::{AppEvent, inventory::FileRow, service};
use tao::event_loop::EventLoopProxy;

/// Accept image MIME types or recognized extensions when MIME metadata is generic.
pub fn supported(row: &FileRow) -> bool {
    row.mime.starts_with("image/")
        || [
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "avif",
        ]
        .iter()
        .any(|ext| row.name.to_ascii_lowercase().ends_with(&format!(".{ext}")))
}

/// Fetch and rasterize one image asynchronously. full selects the larger hover
/// render; epoch and modified are echoed so the frontend can reject stale results.
/// None means unavailable. A browser-level failure (for example the page navigated
/// while the image was loading) is reported as retryable rather than unavailable.
pub fn load(
    view: &wry::WebView,
    session: &service::Session,
    row: &FileRow,
    epoch: u64,
    full: bool,
    proxy: EventLoopProxy<AppEvent>,
) -> Result<(), String> {
    let id = row.id.clone();
    let modified = row.modified.clone();
    let args = serde_json::json!({"id":row.id,"name":row.name,"mime":row.mime,"bytes":row.bytes,"full":full,"authorization":session.authorization,"account":session.account});
    let script = format!(
        r#"(async()=>{{
      if(location.origin!=='https://chatgpt.com')return null;
      const a={args},base='/backend-api/files/library/files/'+encodeURIComponent(a.id),headers={{Authorization:a.authorization}};
      if(a.account)headers['ChatGPT-Account-ID']=a.account;
      async function meta(path){{try{{const r=await fetch(path,{{credentials:'include',headers,signal:AbortSignal.timeout(10000)}});return r.ok?await r.json():null}}catch{{return null}}}}
      async function render(value,limit){{
      if(!value)return null;
      const u=new URL(value,location.origin);if(u.protocol!=='https:'||!(u.hostname==='chatgpt.com'||['.oaiusercontent.com','.blob.core.windows.net','.openai.com','.oaistatic.com'].some(s=>u.hostname.endsWith(s))))return null;
      // Signed remote image URLs receive neither session headers nor browser credentials.
      const own=u.origin===location.origin;
      const r=await fetch(u.href,{{credentials:own?'include':'omit',headers:own?headers:{{}},signal:AbortSignal.timeout(15000)}});
      if(!r.ok||Number(r.headers.get('content-length'))>limit)return null;
      // Content-Length may be absent or wrong, so enforce the cap while streaming as well.
      const reader=r.body.getReader(),chunks=[];let size=0;
      try{{while(true){{const {{done,value}}=await reader.read();if(done)break;size+=value.length;if(size>limit)return null;chunks.push(value)}}}}finally{{await reader.cancel().catch(()=>{{}})}}
      let mime=r.headers.get('content-type')?.split(';')[0]||a.mime;
      // Legacy files sometimes use a generic MIME type; known extensions provide a decode hint.
      if(!mime.startsWith('image/')){{const ext=a.name.split('.').pop().toLowerCase();mime=({{png:'image/png',jpg:'image/jpeg',jpeg:'image/jpeg',ico:'image/x-icon',svg:'image/svg+xml',gif:'image/gif',webp:'image/webp',bmp:'image/bmp',avif:'image/avif'}})[ext]||'';}}
      if(!mime)return null;
      // Blob URLs stay inside this WebView and are revoked even when decoding fails.
      const object=URL.createObjectURL(new Blob(chunks,{{type:mime}}));
      try{{const img=new Image();await new Promise((resolve,reject)=>{{const timer=setTimeout(()=>reject(Error('decode timeout')),10000);img.onload=()=>{{clearTimeout(timer);resolve()}};img.onerror=()=>{{clearTimeout(timer);reject(Error('unsupported image'))}};img.src=object}});
      if(!img.naturalWidth||!img.naturalHeight||img.naturalWidth*img.naturalHeight>100000000)return null;
      // Small tiles are centered in 96 square pixels; hover retains aspect ratio up to 2048.
      // Rasterizing also keeps active vector markup out of the trusted shell DOM.
      const canvas=document.createElement('canvas');const scale=a.full?Math.min(1,2048/img.naturalWidth,2048/img.naturalHeight):Math.min(96/img.naturalWidth,96/img.naturalHeight);const w=Math.max(1,Math.round(img.naturalWidth*scale)),h=Math.max(1,Math.round(img.naturalHeight*scale));canvas.width=a.full?w:96;canvas.height=a.full?h:96;const ctx=canvas.getContext('2d');ctx.drawImage(img,(canvas.width-w)/2,(canvas.height-h)/2,w,h);return canvas.toDataURL('image/png');
      }}finally{{URL.revokeObjectURL(object)}}
      }}
      // An existing thumbnail URL can still be expired, unavailable, or undecodable.
      if(!a.full){{try{{const thumb=(await meta(base+'/thumbnail_url?generate_if_missing=false'))?.thumbnail_url;const data=await render(thumb,4194304);if(data)return data;}}catch{{}}}}
      if(a.bytes!=null&&a.bytes>33554432)return null;
      try{{return await render((await meta(base+'/content_url'))?.content_url,33554432)}}catch{{return null}}
    }})()"#
    );
    service::evaluate_json(view, script, move |result| {
        let interrupted = result.is_err();
        let data = result
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|s| {
                s.starts_with("data:image/png;base64,")
                    && s.len() < if full { 24000000 } else { 131072 }
            });
        let _ = proxy.send_event(AppEvent::PreviewResult(
            epoch,
            id,
            modified,
            data,
            full,
            interrupted,
        ));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::parse_row;
    use serde_json::json;
    #[test]
    fn recognizes_icon_extensions_without_image_mime() {
        for name in ["favicon.ico", "icon.SVG", "photo.PNG"] {
            let f = parse_row(
                &json!({"id":"libfile_a","name":name,"mime_type":"application/octet-stream"}),
            )
            .unwrap();
            assert!(supported(&f));
        }
        let f = parse_row(&json!({"id":"libfile_a","name":"archive.zip"})).unwrap();
        assert!(!supported(&f));
    }
}
