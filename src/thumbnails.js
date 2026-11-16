/**
 * Lazy thumbnail decoration and transient hover previews for inventory.js.
 * Native code fetches/decodes image content; this UI receives bounded PNG data URLs.
 * Small and enlarged images have separate memory caches. No preview is saved to disk.
 * Epoch checks prevent a previous scan/account's async results from reappearing.
 */
const previewStyle=document.createElement('style');previewStyle.textContent=`
td.name button.file-link{display:flex;align-items:center;gap:10px;width:100%;height:34px}.file-label{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.file-visual{display:inline-flex;align-items:center;justify-content:center;flex:0 0 32px;width:32px;height:32px;border-radius:5px;border:1px solid #40535d;background:#24343d;color:#b8ced8;font-size:9px;font-weight:600;letter-spacing:.3px;overflow:hidden}.file-visual.image-type{background-color:#233138;background-image:conic-gradient(#31434b 25%,transparent 0 50%,#31434b 0 75%,transparent 0);background-size:10px 10px}.file-visual img{width:32px;height:32px;object-fit:contain}.file-visual.ready{border-color:#536c76}td.name{padding-top:6px;padding-bottom:6px}
`;document.head.append(previewStyle);
/** Queue at most three small previews in this UI. Native code imposes an additional
 * global outstanding-request bound shared with hover work, including work from old epochs. */
let previewEpoch=0,previewActive=0,previewQueue=[],previewCache=new Map(),previewPending=new Set();
/** Bind cached pixels to both stable file identity and the listed revision timestamp. */
const previewKey=f=>JSON.stringify([f.id,f.modified||'']);
/** Mirror native image eligibility so generic-MIME PNG/JPG/icon files still receive previews. */
function previewEligible(f){return f.mime.startsWith('image/')||/\.(png|jpe?g|gif|webp|bmp|ico|svg|avif)$/i.test(f.name)}
/** Replace the extension fallback only when decoded image data is available; keep filenames as text. */
function applyPreview(el,data){if(!data)return;const image=document.createElement('img');image.alt=el.dataset.name+' thumbnail';image.src=data;el.replaceChildren(image);el.classList.add('ready');el.title=el.dataset.name;}
/** Request rows only near the visible scroll area. Unobserve each queued row to avoid
 * duplicate requests; cache hits require no IPC or network fetch. */
const previewObserver=new IntersectionObserver(entries=>{for(const entry of entries){if(!entry.isIntersecting)continue;const el=entry.target;previewObserver.unobserve(el);const key=el.dataset.key;if(previewCache.has(key)){applyPreview(el,previewCache.get(key));continue;}if(!previewPending.has(key)){previewPending.add(key);previewQueue.push({id:el.dataset.id,modified:el.dataset.modified||null,key});}}pumpPreviews();},{root:document.querySelector('.tablewrap'),rootMargin:'80px'});
/** Drop queued work for rows removed by sorting/filtering, then fill the concurrency slots. */
function pumpPreviews(){while(previewActive<3&&previewQueue.length){const p=previewQueue.shift();if(!document.querySelector(`.file-visual[data-id="${CSS.escape(p.id)}"]`)){previewPending.delete(p.key);continue;}previewActive++;send('thumbnail:'+JSON.stringify({id:p.id,modified:p.modified,epoch:previewEpoch}));}}
/** Rebuild thumbnail boxes after the table replaces its rows. Discard obsolete queued
 * DOM work, retain in-flight requests, and resume observation only after a completed scan. */
function decoratePreviews(){previewObserver.disconnect();for(const p of previewQueue)previewPending.delete(p.key);previewQueue=[];const files=filteredFiles().slice(tablePage*PAGE_SIZE,(tablePage+1)*PAGE_SIZE);const rows=[...document.querySelectorAll('#filebody tr')];for(let i=0;i<files.length;i++){const f=files[i],button=rows[i]?.querySelector('td.name button');if(!button)continue;const box=document.createElement('span');box.className='file-visual';box.addEventListener('click',e=>{e.preventDefault();e.stopPropagation();});box.dataset.id=f.id;box.dataset.name=f.name;box.dataset.modified=f.modified||'';box.dataset.key=previewKey(f);const image=previewEligible(f);if(image)box.classList.add('image-type');const ext=f.name.includes('.')?f.name.split('.').pop().slice(0,4).toUpperCase():'FILE';box.textContent=ext;box.title=image?'Image preview loading':f.mime||ext;const label=document.createElement('span');label.className='file-label';label.textContent=f.name+(f.action_blocked?' (refresh to verify)':'');button.classList.add('file-link');button.replaceChildren(box,label);if(image){if(previewCache.has(box.dataset.key)){applyPreview(box,previewCache.get(box.dataset.key));if(!previewCache.get(box.dataset.key))box.title='Small preview unavailable';}else if(!inventoryState.busy&&inventoryState.complete)previewObserver.observe(box);}}}
/** Handle native results. A busy response schedules another observation without caching
 * a false unavailable result; a real failure may be cached until eviction or refresh.
 * Old epochs are ignored before touching current counters or caches. */
window.showPreview=r=>{if(r.full){showHoverPreview(r);return;}if(r.epoch!==previewEpoch)return;previewActive=Math.max(0,previewActive-1);const key=previewKey(r);previewPending.delete(key);if(r.retry){const epoch=previewEpoch;setTimeout(()=>{if(epoch!==previewEpoch)return;for(const el of document.querySelectorAll('.file-visual'))if(el.dataset.key===key)previewObserver.observe(el);pumpPreviews();},300);pumpPreviews();return;}previewCache.set(key,r.data||null);while(previewCache.size>300)previewCache.delete(previewCache.keys().next().value);for(const el of document.querySelectorAll('.file-visual'))if(el.dataset.key===key){applyPreview(el,r.data);if(!r.data)el.title='Small preview unavailable';}pumpPreviews();};
/** Reset UI state on a new scan/session. This does not cancel native fetches, whose
 * outstanding count must remain intact until their callbacks complete. */
window.clearPreviews=()=>{hideHoverPreview();hoverCache.clear();hoverPending.clear();previewEpoch++;previewActive=0;previewQueue=[];previewPending.clear();previewCache.clear();previewObserver.disconnect();};
/** Wrap the inventory/action renderer, preserving its behavior before adding visual cells. */
const renderWithPreviews=renderFiles;renderFiles=()=>{renderWithPreviews();decoratePreviews()};
decoratePreviews();

// Hover stays in the file table: no navigation, focus change, or dismissal click.
const hoverStyle=document.createElement('style');hoverStyle.textContent=`
#hover-preview{position:fixed;z-index:10000;pointer-events:none;max-width:70vw;max-height:85vh;padding:10px;background:#17252ef5;border:1px solid #64808e;border-radius:10px;box-shadow:0 12px 40px #0009;color:#dceaf1;overflow:hidden}
#hover-preview[hidden]{display:none}#hover-preview img{display:block;object-fit:contain;max-width:calc(70vw - 22px);max-height:calc(85vh - 54px)}#hover-preview .caption{font-size:12px;margin-top:7px;max-width:65vw;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
`;document.head.append(hoverStyle);
/** One reusable overlay, with pointer-events disabled so it cannot steal hover from its anchor. */
const hoverPanel=document.createElement('div');hoverPanel.id='hover-preview';hoverPanel.hidden=true;hoverPanel.setAttribute('role','tooltip');document.body.append(hoverPanel);
let hoverTarget=null,hoverTimer=null,hoverCache=new Map(),hoverPending=new Set();
/** Cancel the hover delay and remove the anchor; late responses may cache but cannot reopen it. */
function hideHoverPreview(){clearTimeout(hoverTimer);hoverTarget=null;hoverPanel.hidden=true;hoverPanel.replaceChildren();}
/** Prefer the right of the thumbnail, then clamp within the current viewport.
 * A detached anchor means the table was rebuilt and the overlay must disappear. */
function placeHoverPreview(){if(!hoverTarget?.isConnected){hideHoverPreview();return;}const r=hoverTarget.getBoundingClientRect(),p=hoverPanel.getBoundingClientRect();let x=r.right+16;if(x+p.width>innerWidth-12)x=r.left-p.width-16;x=Math.max(12,Math.min(x,innerWidth-p.width-12));let y=Math.max(12,Math.min(r.top-12,innerHeight-p.height-12));hoverPanel.style.left=x+'px';hoverPanel.style.top=y+'px';}
/** Show the loaded image or an unavailable message; reposition again after image dimensions resolve. */
function paintHoverPreview(data){if(!hoverTarget)return;hoverPanel.replaceChildren();if(data){const img=document.createElement('img');img.alt=hoverTarget.dataset.name+' enlarged preview';img.onload=placeHoverPreview;img.src=data;hoverPanel.append(img);}const label=document.createElement('div');label.className='caption';label.textContent=data?hoverTarget.dataset.name:'Larger preview unavailable';hoverPanel.append(label);hoverPanel.hidden=false;placeHoverPreview();}
/** Cache at most four enlarged images. Retry capacity failures only while the same
 * thumbnail is still hovered and the request belongs to the current epoch. */
function showHoverPreview(r){if(r.epoch!==previewEpoch)return;const key=previewKey(r);hoverPending.delete(key);if(r.retry){const epoch=previewEpoch;setTimeout(()=>{if(epoch!==previewEpoch||hoverTarget?.dataset.key!==key||hoverPending.has(key))return;hoverPending.add(key);send('thumbnail:'+JSON.stringify({id:r.id,modified:r.modified,epoch,full:true}));},300);return;}hoverCache.set(key,r.data||null);while(hoverCache.size>4)hoverCache.delete(hoverCache.keys().next().value);if(hoverTarget?.dataset.key===key)paintHoverPreview(r.data);}
/** Delay fetching briefly so moving across rows does not immediately load every original image. */
document.addEventListener('pointerover',e=>{const el=e.target.closest('.file-visual.image-type');if(!el||el===hoverTarget)return;hideHoverPreview();hoverTarget=el;hoverTimer=setTimeout(()=>{if(hoverTarget!==el)return;const key=el.dataset.key;if(hoverCache.has(key)){paintHoverPreview(hoverCache.get(key));return;}hoverPanel.textContent='Loading larger image...';hoverPanel.hidden=false;placeHoverPreview();if(!hoverPending.has(key)){hoverPending.add(key);send('thumbnail:'+JSON.stringify({id:el.dataset.id,modified:el.dataset.modified||null,epoch:previewEpoch,full:true}));}},200);});
/** Leaving the thumbnail dismisses the overlay, including while an image is still loading. */
document.addEventListener('pointerout',e=>{if(hoverTarget&&hoverTarget.contains(e.target)&&!hoverTarget.contains(e.relatedTarget))hideHoverPreview();});
/** Scrolling, losing focus, or resizing invalidates the overlay position and hover context. */
document.addEventListener('scroll',hideHoverPreview,true);window.addEventListener('blur',hideHoverPreview);window.addEventListener('resize',hideHoverPreview);
/** The outermost renderer closes hover before any table nodes are replaced. */
const renderBeforeHover=renderFiles;renderFiles=()=>{hideHoverPreview();renderBeforeHover();};
