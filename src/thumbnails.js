/**
 * Lazy thumbnails and transient hover previews for the file table (after inventory.js).
 * Native code fetches/decodes image content; this UI receives bounded PNG data URLs.
 * Small and enlarged images have separate memory caches. No preview is saved to disk.
 * Epoch checks prevent a previous scan/account's async results from reappearing.
 */

/** Rows request small previews through at most PREVIEW_SLOTS concurrent native loads.
 * Native code enforces its own higher bound, shared with hover work and old epochs. */
const PREVIEW_SLOTS = 6, PREVIEW_RETRIES = 6;
let previewEpoch = 0, previewActive = 0, previewQueue = [];
const previewCache = new Map(), previewPending = new Set(), previewRetries = new Map();
/** Bind cached pixels to both stable file identity and the listed revision timestamp. */
const previewKey = f => JSON.stringify([f.id, f.modified || '']);
/** Mirror native image eligibility so generic-MIME PNG/JPG/icon files still receive previews. */
function previewEligible(f) {
  return f.mime.startsWith('image/') || /\.(png|jpe?g|gif|webp|bmp|ico|svg|avif)$/i.test(f.name);
}
/** Replace the extension fallback only when decoded image data is available; keep filenames as text. */
function applyPreview(el, data) {
  if (!data) return;
  const image = document.createElement('img');
  image.alt = el.dataset.name + ' thumbnail';
  image.src = data;
  el.replaceChildren(image);
  el.classList.add('ready');
  el.title = el.dataset.name;
}
/** Request rows only near the visible scroll area. Unobserve each queued row to avoid
 * duplicate requests; cache hits require no IPC or network fetch. */
const previewObserver = new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue;
    const el = entry.target, key = el.dataset.key;
    previewObserver.unobserve(el);
    if (previewCache.has(key)) { applyPreview(el, previewCache.get(key)); continue; }
    if (previewPending.has(key)) continue;
    previewPending.add(key);
    previewQueue.push({id: el.dataset.id, modified: el.dataset.modified || null, key});
  }
  pumpPreviews();
}, {root: $('table-wrap'), rootMargin: '120px'});
/** Drop queued work for rows removed by sorting/filtering, then fill the concurrency slots. */
function pumpPreviews() {
  while (previewActive < PREVIEW_SLOTS && previewQueue.length) {
    const p = previewQueue.shift();
    if (!document.querySelector(`.file-visual[data-id="${CSS.escape(p.id)}"]`)) {
      previewPending.delete(p.key);
      continue;
    }
    previewActive++;
    send('thumbnail:' + JSON.stringify({id: p.id, modified: p.modified, epoch: previewEpoch}));
  }
}
/** Every drawn thumbnail box for one file revision. */
const visualsFor = key => [...document.querySelectorAll('.file-visual')].filter(el => el.dataset.key === key);
/** Handle native results. A busy or interrupted load is retried a bounded number of times
 * instead of being cached as unavailable; a real failure is cached until refresh.
 * Old epochs are ignored before touching current counters or caches. */
window.showPreview = r => {
  if (r.full) { showHoverPreview(r); return; }
  if (r.epoch !== previewEpoch) return;
  previewActive = Math.max(0, previewActive - 1);
  const key = previewKey(r), tries = (previewRetries.get(key) || 0) + 1;
  previewPending.delete(key);
  if (r.retry && tries <= PREVIEW_RETRIES) {
    previewRetries.set(key, tries);
    const epoch = previewEpoch;
    setTimeout(() => {
      if (epoch !== previewEpoch) return;
      for (const el of visualsFor(key)) previewObserver.observe(el);
    }, 300);
    pumpPreviews();
    return;
  }
  previewRetries.delete(key);
  previewCache.set(key, r.data || null);
  while (previewCache.size > 300) previewCache.delete(previewCache.keys().next().value);
  for (const el of visualsFor(key)) {
    applyPreview(el, r.data);
    if (!r.data) el.title = 'Preview unavailable';
  }
  pumpPreviews();
};
/** Reset UI state on a new scan/session. This does not cancel native fetches, whose
 * outstanding count must remain intact until their callbacks complete. */
window.clearPreviews = () => {
  hideHoverPreview();
  hoverCache.clear();
  hoverPending.clear();
  previewEpoch++;
  previewActive = 0;
  previewQueue = [];
  previewPending.clear();
  previewRetries.clear();
  previewCache.clear();
  previewObserver.disconnect();
};
// Before the table is rebuilt: forget queued (not yet requested) rows; in-flight loads continue.
renderHooks.before.push(() => {
  previewObserver.disconnect();
  for (const p of previewQueue) previewPending.delete(p.key);
  previewQueue = [];
});
// Each new row: tag its thumbnail box, then paint from cache or wait until it scrolls into
// view. Loading starts only after a completed listing, so a scan is never slowed by images.
renderHooks.row.push((box, f) => {
  box.dataset.id = f.id;
  box.dataset.name = f.name;
  box.dataset.modified = f.modified || '';
  box.dataset.key = previewKey(f);
  if (!previewEligible(f)) { box.title = f.mime; return; }
  box.classList.add('image-type');
  const store = stores[viewMode];
  if (previewCache.has(box.dataset.key)) {
    applyPreview(box, previewCache.get(box.dataset.key));
    if (!previewCache.get(box.dataset.key)) box.title = 'Preview unavailable';
  } else if (!store.busy && store.complete) {
    previewObserver.observe(box);
  }
});

// #region hover
// Hover stays in the file table: no navigation, focus change, or dismissal click.
/** One reusable overlay, with pointer-events disabled so it cannot steal hover from its anchor. */
const hoverPanel = document.createElement('div');
hoverPanel.id = 'hover-preview';
hoverPanel.hidden = true;
hoverPanel.setAttribute('role', 'tooltip');
document.body.append(hoverPanel);
const HOVER_RETRIES = 10;
let hoverTarget = null, hoverTimer = null, hoverTries = 0;
const hoverCache = new Map(), hoverPending = new Set();
/** Cancel the hover delay and remove the anchor; late responses may cache but cannot reopen it. */
function hideHoverPreview() {
  clearTimeout(hoverTimer);
  hoverTarget = null;
  hoverPanel.hidden = true;
  hoverPanel.replaceChildren();
}
/** Prefer the right of the thumbnail, then clamp within the current viewport.
 * A detached anchor means the table was rebuilt and the overlay must disappear. */
function placeHoverPreview() {
  if (!hoverTarget?.isConnected) { hideHoverPreview(); return; }
  const r = hoverTarget.getBoundingClientRect(), p = hoverPanel.getBoundingClientRect();
  let x = r.right + 16;
  if (x + p.width > innerWidth - 12) x = r.left - p.width - 16;
  x = Math.max(12, Math.min(x, innerWidth - p.width - 12));
  const y = Math.max(12, Math.min(r.top - 12, innerHeight - p.height - 12));
  hoverPanel.style.left = x + 'px';
  hoverPanel.style.top = y + 'px';
}
/** Show the loaded image or an unavailable message; reposition again after image dimensions resolve. */
function paintHoverPreview(data) {
  if (!hoverTarget) return;
  hoverPanel.replaceChildren();
  if (data) {
    const img = document.createElement('img');
    img.alt = hoverTarget.dataset.name + ' enlarged preview';
    img.onload = placeHoverPreview;
    img.src = data;
    hoverPanel.append(img);
  }
  const label = document.createElement('div');
  label.className = 'caption';
  label.textContent = data ? hoverTarget.dataset.name : 'This image could not be previewed';
  hoverPanel.append(label);
  hoverPanel.hidden = false;
  placeHoverPreview();
}
/** Cache at most four enlarged images. Retry busy or interrupted loads only while the same
 * thumbnail is still hovered and the request belongs to the current epoch. */
function showHoverPreview(r) {
  if (r.epoch !== previewEpoch) return;
  const key = previewKey(r);
  hoverPending.delete(key);
  if (r.retry && hoverTries < HOVER_RETRIES) {
    const epoch = previewEpoch;
    setTimeout(() => {
      if (epoch !== previewEpoch || hoverTarget?.dataset.key !== key || hoverPending.has(key)) return;
      hoverTries++;
      hoverPending.add(key);
      send('thumbnail:' + JSON.stringify({id: r.id, modified: r.modified, epoch, full: true}));
    }, 300);
    return;
  }
  hoverCache.set(key, r.data || null);
  while (hoverCache.size > 4) hoverCache.delete(hoverCache.keys().next().value);
  if (hoverTarget?.dataset.key === key) paintHoverPreview(r.data);
}
/** Delay fetching briefly so moving across rows does not immediately load every original image. */
document.addEventListener('pointerover', e => {
  const el = e.target.closest('.file-visual.image-type');
  if (!el || el === hoverTarget) return;
  hideHoverPreview();
  hoverTarget = el;
  hoverTries = 0;
  hoverTimer = setTimeout(() => {
    if (hoverTarget !== el) return;
    const key = el.dataset.key;
    if (hoverCache.has(key)) { paintHoverPreview(hoverCache.get(key)); return; }
    hoverPanel.textContent = 'Loading preview…';
    hoverPanel.hidden = false;
    placeHoverPreview();
    if (!hoverPending.has(key)) {
      hoverPending.add(key);
      send('thumbnail:' + JSON.stringify({id: el.dataset.id, modified: el.dataset.modified || null, epoch: previewEpoch, full: true}));
    }
  }, 200);
});
/** Leaving the thumbnail dismisses the overlay, including while an image is still loading. */
document.addEventListener('pointerout', e => {
  if (hoverTarget && hoverTarget.contains(e.target) && !hoverTarget.contains(e.relatedTarget)) hideHoverPreview();
});
/** Scrolling, losing focus, or resizing invalidates the overlay position and hover context. */
document.addEventListener('scroll', hideHoverPreview, true);
window.addEventListener('blur', hideHoverPreview);
window.addEventListener('resize', hideHoverPreview);
/** Close the overlay before any table rows are replaced. */
renderHooks.before.push(hideHoverPreview);
// #endregion hover
