/**
 * Trusted local file-browser UI; loaded inline by ui.html, before thumbnails.js.
 * Rust owns authentication, authoritative snapshots, review validation, and mutations.
 * This script owns display state and sends only commands/IDs through send().
 * Filenames and other service values enter DOM text nodes, never interpolated HTML.
 *
 * Files and Trash are separate stores. Rust replaces a store with a full snapshot
 * (showInventory/showTrash) or patches it (patchView); filtering and sorting always
 * cover the complete store, and PAGE_SIZE only limits how many rows are drawn.
 *
 * The "#region" markers delimit self-contained logic that tests/preflight.cjs runs
 * directly from this file. Keep each region free of references to later code.
 */
const $ = id => document.getElementById(id);

// #region format
/** Format numeric bytes for display only. Sorting and selection totals retain numeric values. */
function bytesText(n) {
  if (n == null) return 'Unknown';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let k = 0;
  // Step up a unit at 1,000 so a value never needs four digits (0.99 GB, not 1,016 MB).
  while (n >= 1000 && k < units.length - 1) { n /= 1024; k++; }
  // Three significant digits: 4.82 MB, 96.3 GB, 461 MB. Whole bytes are never fractional.
  const digits = k === 0 || n >= 100 ? 0 : n >= 10 ? 1 : 2;
  return n.toLocaleString(undefined, {maximumFractionDigits: digits}) + ' ' + units[k];
}
/** Classify for UI filtering; extension fallback supports legacy images with generic MIME metadata. */
function fileType(f) {
  if (f.mime.startsWith('image/') || /\.(png|jpe?g|gif|webp|bmp|ico|svg|avif)$/i.test(f.name)) return 'image';
  if (/\.(zip|xz|gz|tar|7z|rar|bz2|zst)$/i.test(f.name)) return 'archive';
  if (/\.(pdf|docx?|xlsx?|csv|txt|md|pptx?|json|html?)$/i.test(f.name) || f.mime.startsWith('text/')) return 'document';
  return 'other';
}
/** Finer grouping for the name-column glyph only; the type filter keeps fileType's groups. */
function fileKind(f) {
  if (f.mime.startsWith('video/') || /\.(mp4|mov|webm|mkv|avi|m4v)$/i.test(f.name)) return 'video';
  if (f.mime.startsWith('audio/') || /\.(mp3|wav|m4a|flac|ogg|aac)$/i.test(f.name)) return 'audio';
  if (/\.(csv|tsv|xlsx?)$/i.test(f.name)) return 'sheet';
  return fileType(f);
}
/** Derive the values that filtering and sorting read for every row, once per row. */
function prepare(row) {
  row._name = row.name.toLowerCase();
  const time = row.modified ? Date.parse(row.modified) : NaN;
  row._time = Number.isFinite(time) ? time : null;
  row._type = row.folder ? 'folder' : fileType(row);
  return row;
}
// #endregion format

/** Changing the visible page must not limit selection, filtering, or sorting to that page. */
const PAGE_SIZE = 100;
const makeStore = () => ({rows: new Map(), version: 0, busy: false, complete: false, error: null, pages: 0});
/** Files and Trash keep separate snapshots; `version` changes whenever a store's rows change. */
const stores = {files: makeStore(), trash: makeStore()};
/** View state is shared by both stores; selection holds stable IDs rather than row indexes. */
let viewMode = 'files', selected = new Set(), sortKey = 'bytes', sortDirection = -1, tablePage = 0;
let jobActive = false;

// #region queue
// Confirmed reservations are presentation-only: native snapshots retain these rows
// until readback verifies a change. Stopped/skipped work can therefore reappear.
let reservedIds = new Set(), reservedVersion = 0, queuedBatchCount = 0, queuedFileCount = 0;
window.showQueue = data => {
  reservedIds = new Set(data.ids);
  reservedVersion++;
  queuedBatchCount = data.batches;
  queuedFileCount = data.files;
  for (const id of reservedIds) selected.delete(id);
  if (reservedIds.has(selectionAnchor)) selectionAnchor = null;
  renderFiles();
};
// #endregion queue

// #region selection
let selectionAnchor = null;
/** Apply Shift-click against the full filtered/sorted ordering, skipping quarantined rows.
 * Keep the original anchor through repeated Shift-clicks; a missing anchor starts a new range. */
function selectRange(files, id, checked, shift) {
  const index = files.findIndex(f => f.id === id), anchor = files.findIndex(f => f.id === selectionAnchor);
  if (index < 0) return;
  if (shift && anchor >= 0) {
    for (const f of files.slice(Math.min(index, anchor), Math.max(index, anchor) + 1)) {
      if (f.action_blocked) continue;
      if (checked) selected.add(f.id); else selected.delete(f.id);
    }
  } else {
    if (!files[index].action_blocked) {
      if (checked) selected.add(id); else selected.delete(id);
    }
    selectionAnchor = id;
  }
}
// #endregion selection

// #region filter
const nameOrder = new Intl.Collator(undefined, {numeric: true, sensitivity: 'base'});
const idOrder = (a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
/** Unknown values stay last and the stable ID breaks ties, so pagination is deterministic. */
function compareRows(a, b) {
  const av = sortKey === 'bytes' ? a.bytes : sortKey === 'modified' ? a._time : a.name;
  const bv = sortKey === 'bytes' ? b.bytes : sortKey === 'modified' ? b._time : b.name;
  if (av == null) return bv == null ? idOrder(a, b) : 1;
  if (bv == null) return -1;
  const result = sortKey === 'name' ? nameOrder.compare(av, bv) : av - bv;
  return result * sortDirection || idOrder(a, b);
}
/** The chosen folder plus every folder beneath it. Visiting each ID once keeps cyclic
 * folder metadata from hanging the UI. Resolved locally; no network request is issued. */
function folderScope(store, rootId) {
  const children = new Map();
  for (const row of store.rows.values()) {
    if (!row.folder) continue;
    if (!children.has(row.parent)) children.set(row.parent, []);
    children.get(row.parent).push(row.id);
  }
  const scope = new Set([rootId]), pending = [rootId];
  while (pending.length) {
    for (const child of children.get(pending.pop()) || []) {
      if (!scope.has(child)) { scope.add(child); pending.push(child); }
    }
  }
  return scope;
}
let filterCache = {key: null, files: []};
/** Filter and sort the entire current store. The result is reused until the rows,
 * reservations, filters, or sort order change, so selection clicks never re-sort. */
function filteredFiles() {
  const store = stores[viewMode];
  const query = $('findfiles').value.toLowerCase(), type = $('typefilter').value;
  const folder = viewMode === 'files' ? $('folderfilter').value : '';
  const key = [viewMode, store.version, reservedVersion, sortKey, sortDirection, type, folder, query].join('\u0001');
  if (filterCache.key === key) return filterCache.files;
  const scope = folder ? folderScope(store, folder) : null;
  const files = [];
  for (const f of store.rows.values()) {
    if (f.folder || reservedIds.has(f.id)) continue;
    if (query && !f._name.includes(query)) continue;
    if (type !== 'all' && f._type !== type) continue;
    if (scope && !scope.has(f.parent)) continue;
    files.push(f);
  }
  files.sort(compareRows);
  filterCache = {key, files};
  return files;
}
// #endregion filter

/** thumbnails.js registers here instead of wrapping renderFiles: `before` runs ahead of
 * a table rebuild, `row` receives each new thumbnail box, `after` runs once rows exist. */
const renderHooks = {before: [], row: [], after: []};

/** Reference a glyph from the sprite in ui.html: "i" for interface icons, "k" for file kinds. */
const icon = (name, set = 'i') => {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'icon');
  svg.append(document.createElementNS('http://www.w3.org/2000/svg', 'use'));
  svg.firstChild.setAttribute('href', `#${set}-${name}`);
  return svg;
};
const whenText = f => f._time == null ? 'Unknown' : new Date(f._time).toLocaleString(undefined, {dateStyle: 'medium', timeStyle: 'short'});

/** One table row. Text enters through textContent; events are delegated to the tbody. */
function buildRow(f) {
  const tr = document.createElement('tr');
  tr.dataset.id = f.id;
  const pick = tr.insertCell(), check = document.createElement('input');
  pick.className = 'pick';
  check.type = 'checkbox';
  check.disabled = !!f.action_blocked;
  check.setAttribute('aria-label', 'Select ' + f.name);
  pick.append(check);

  const name = tr.insertCell(), link = document.createElement('button');
  const visual = document.createElement('span'), label = document.createElement('span');
  name.className = 'name';
  link.type = 'button';
  link.className = 'file-link';
  const kind = fileKind(f);
  visual.className = 'file-visual ' + kind;
  visual.append(icon(kind, 'k'));
  label.className = 'file-label';
  label.textContent = f.name;
  link.append(visual, label);
  link.title = viewMode === 'trash' ? 'Restore this file to open it in ChatGPT.' : f.name;
  if (f.action_blocked) {
    const flag = document.createElement('span');
    flag.className = 'file-flag';
    flag.textContent = 'Needs refresh';
    link.append(flag);
    link.title = 'Refresh to verify this file before changing it again.';
  }
  name.append(link);

  const when = tr.insertCell(), size = tr.insertCell();
  when.className = f._time == null ? 'when unknown' : 'when';
  when.textContent = whenText(f);
  when.title = f.modified || '';
  size.className = f.bytes == null ? 'size unknown' : 'size';
  size.textContent = bytesText(f.bytes);
  size.title = f.bytes == null ? 'Size not supplied' : f.bytes.toLocaleString() + ' bytes';
  for (const hook of renderHooks.row) hook(visual, f);
  return tr;
}

/** What the table currently shows, so an update that leaves the visible page untouched
 * (a change elsewhere in the collection, or in the other view) does not rebuild it. */
let drawn = {state: '', rows: []};
/** Draw the visible page. Selection totals and select-all state are computed over the
 * whole filtered collection in paintSelection, never just the rows drawn here. */
function renderFiles() {
  const store = stores[viewMode], files = filteredFiles();
  const pages = Math.max(1, Math.ceil(files.length / PAGE_SIZE));
  tablePage = Math.max(0, Math.min(tablePage, pages - 1));
  const first = tablePage * PAGE_SIZE, visible = files.slice(first, first + PAGE_SIZE);
  // Thumbnails start loading once a listing completes, so that state is part of the picture.
  const state = `${viewMode}|${store.busy}|${store.complete}`;
  const unchanged = drawn.state === state && drawn.rows.length === visible.length
    && visible.every((f, i) => f === drawn.rows[i]);
  if (!unchanged) {
    for (const hook of renderHooks.before) hook();
    const rows = document.createDocumentFragment();
    for (const f of visible) rows.append(buildRow(f));
    $('filebody').replaceChildren(rows);
    drawn = {state, rows: visible};
    for (const hook of renderHooks.after) hook();
  }

  $('empty').hidden = files.length > 0;
  if (!files.length) {
    const listed = store.rows.size > 0, trash = viewMode === 'trash';
    $('empty-title').textContent = store.busy ? (trash ? 'Loading Trash…' : 'Scanning your Library…')
      : listed ? 'No matching files'
      : trash ? (store.complete ? 'Trash is empty' : 'Trash is not loaded')
      : 'No files listed yet';
    $('empty-text').textContent = store.busy ? 'Files appear here as they are found.'
      : listed ? 'Try a different search or filter.'
      : trash ? (store.complete ? 'Files you move to Trash appear here.' : 'Scan your Library first, then open Trash again.')
      : 'Scan your Library to list, sort, and search every file.';
    // Offer the one useful next step where the eye already is: undo the filters that
    // hide everything, or start the listing that has not run yet.
    const action = $('empty-action');
    action.hidden = store.busy || (listed ? false : store.complete || jobActive);
    action.textContent = listed ? 'Clear filters' : trash ? 'Load Trash' : 'Scan all files';
    action.className = listed ? 'btn' : 'btn primary';
  }
  $('pageno').textContent = files.length
    ? `${(first + 1).toLocaleString()}–${(first + visible.length).toLocaleString()} of ${files.length.toLocaleString()}`
    : '';
  $('prevpage').disabled = tablePage === 0;
  $('nextpage').disabled = tablePage + 1 >= pages;
  for (const th of document.querySelectorAll('th[data-sort]')) {
    if (th.dataset.sort === sortKey) th.setAttribute('aria-sort', sortDirection === 1 ? 'ascending' : 'descending');
    else th.removeAttribute('aria-sort');
  }
  paintSummary();
  paintSelection();
}

/** Title, counts, listing state, and the folder filter for the current view. */
function paintSummary() {
  const store = stores[viewMode], trash = viewMode === 'trash';
  let files = 0, folders = 0, bytes = 0;
  for (const row of store.rows.values()) {
    if (row.folder) folders++; else { files++; bytes += row.bytes || 0; }
  }
  const plural = (n, word) => `${n.toLocaleString()} ${word}${n === 1 ? '' : 's'}`;
  const parts = [plural(files, 'file'), bytesText(bytes)];
  if (folders) parts.push(plural(folders, 'folder'));
  $('inventory').classList.toggle('trash', trash);
  $('view-title').textContent = trash ? 'Trash' : 'All files';
  $('view-meta').textContent = store.busy
    ? (trash ? `Loading… ${plural(files, 'file')}` : `Scanning… ${plural(files, 'file')} · ${plural(store.pages, 'page')}`)
    : store.rows.size || store.complete ? parts.join(' · ') : '';
  $('notice').hidden = !store.error;
  $('notice-text').textContent = store.error || '';
  $('activity').hidden = !store.busy;

  // Counts stay visible for both views so the sidebar reflects moves between them.
  // Trash is listed separately, so its count appears only once that listing is complete.
  for (const [id, view, known] of [
    ['count-files', stores.files, stores.files.rows.size > 0 || stores.files.complete],
    ['count-trash', stores.trash, stores.trash.complete],
  ]) {
    let n = 0;
    for (const row of view.rows.values()) if (!row.folder) n++;
    $(id).textContent = known ? n.toLocaleString() : '';
  }

  // Rebuild folder options only when the folder set changes; label nested folders by path.
  const select = $('folderfilter');
  select.disabled = trash;
  if (!trash) {
    const byId = store.rows, list = [];
    for (const row of byId.values()) {
      if (!row.folder) continue;
      const names = [row.name], seen = new Set([row.id]);
      for (let p = byId.get(row.parent); p && p.folder && !seen.has(p.id); p = byId.get(p.parent)) {
        names.unshift(p.name);
        seen.add(p.id);
      }
      list.push([row.id, names.join(' / ')]);
    }
    list.sort((a, b) => nameOrder.compare(a[1], b[1]));
    const signature = JSON.stringify(list);
    if (select.dataset.signature !== signature) {
      const chosen = select.value;
      select.dataset.signature = signature;
      select.replaceChildren(new Option('All folders', ''), ...list.map(([id, path]) => new Option(path, id)));
      // A folder that no longer exists falls back to the unfiltered list.
      select.value = list.some(([id]) => id === chosen) ? chosen : '';
    }
  }
  paintJob();
}

/** Reflect the selection in the drawn rows, the footer, and the select-all box without
 * rebuilding the table, so thumbnails and scroll position are untouched by a click. */
function paintSelection() {
  const store = stores[viewMode];
  for (const tr of $('filebody').rows) {
    const on = selected.has(tr.dataset.id);
    tr.classList.toggle('selected', on);
    tr.cells[0].firstChild.checked = on;
  }
  let count = 0, bytes = 0, unknown = 0;
  for (const id of selected) {
    const f = store.rows.get(id);
    if (!f) continue;
    count++;
    if (f.bytes == null) unknown++; else bytes += f.bytes;
  }
  const label = $('selectioncount');
  label.textContent = count
    ? `${count.toLocaleString()} selected · ${bytesText(bytes)}${unknown ? ` + ${unknown} of unknown size` : ''}`
    : 'No files selected';
  label.classList.toggle('has', count > 0);
  $('clearselection').hidden = !count;

  // Blocked rows are visible for diagnosis but never included by select-all.
  let selectable = 0, chosen = 0, any = false;
  for (const f of filteredFiles()) {
    const on = selected.has(f.id);
    any ||= on;
    if (f.action_blocked) continue;
    selectable++;
    if (on) chosen++;
  }
  const all = $('selectfiltered');
  all.checked = selectable > 0 && chosen === selectable;
  all.indeterminate = !all.checked && any;
  all.disabled = !selectable;
  updateActions(count);
}

/** Reflect completeness, busy state, selection, and current view in the controls.
 * These are usability guards only; the native command handler enforces the same constraints. */
function updateActions(count) {
  const store = stores[viewMode], trash = viewMode === 'trash';
  if (count === undefined) {
    count = 0;
    for (const id of selected) if (store.rows.has(id)) count++;
  }
  const review = $('review-selection');
  review.disabled = !count || !store.complete || store.busy;
  review.classList.toggle('danger-outline', !trash);
  $('review-icon').setAttribute('href', trash ? '#i-restore' : '#i-trash');
  $('review-label').textContent = jobActive
    ? (trash ? 'Add to restore queue…' : 'Add to Trash queue…')
    : (trash ? 'Restore…' : 'Move to Trash…');
  $('scanlabel').textContent = trash ? (store.complete ? 'Refresh' : 'Load Trash')
    : store.rows.size ? 'Refresh' : 'Scan all files';
  $('scanall').disabled = store.busy || jobActive;
  // A running scan offers Stop in the same place; Trash loading cannot be cancelled.
  $('scanall').hidden = !trash && store.busy;
  // Emphasize the listing button only while it is the step that unblocks everything else:
  // an incomplete list that still has rows. An empty list offers the same step in place.
  $('scanall').classList.toggle('primary', !store.complete && !store.busy && store.rows.size > 0);
  $('cancelscan').hidden = trash || !store.busy;
  $('exportcsv').hidden = trash;
  let exportable = false;
  for (const row of stores.files.rows.values()) if (!row.folder) { exportable = true; break; }
  $('exportcsv').disabled = !exportable;
}

// Listing pages can arrive many times a second. Redraw at most every RENDER_INTERVAL
// while a listing is running, and immediately once it settles.
const RENDER_INTERVAL = 200;
let renderTimer = null, lastRender = 0;
function requestRender() {
  const busy = stores[viewMode].busy;
  if (renderTimer !== null) {
    if (busy) return;
    clearTimeout(renderTimer);
  }
  const wait = busy ? Math.max(0, RENDER_INTERVAL - (performance.now() - lastRender)) : 0;
  renderTimer = setTimeout(() => {
    renderTimer = null;
    lastRender = performance.now();
    renderFiles();
  }, wait);
}

/** A store changed: drop selections that no longer exist or became blocked, then redraw
 * if that store is on screen. The other view only needs its sidebar count refreshed. */
function storeChanged(view) {
  if (view === viewMode) {
    const rows = stores[view].rows;
    for (const id of selected) {
      const row = rows.get(id);
      if (!row || row.action_blocked) selected.delete(id);
    }
  }
  requestRender();
}
function applyState(store, data) {
  for (const key of ['busy', 'complete', 'error', 'pages']) if (key in data) store[key] = data[key];
}
function replaceStore(view, data) {
  const store = stores[view];
  store.rows = new Map(data.rows.map(row => [row.id, prepare(row)]));
  store.version++;
  applyState(store, data);
  storeChanged(view);
}
/** Authoritative snapshots: the start and end of a listing, and sign-in changes. */
window.showInventory = data => replaceStore('files', data);
window.showTrash = data => replaceStore('trash', data);
/** Incremental update: listing pages while a scan runs, and each verified file change. */
window.patchView = data => {
  const store = stores[data.view];
  if (!store) return;
  for (const row of data.upsert) store.rows.set(row.id, prepare(row));
  for (const id of data.remove) store.rows.delete(id);
  if (data.upsert.length || data.remove.length) store.version++;
  applyState(store, data);
  storeChanged(data.view);
};

// ---- Sections and navigation ------------------------------------------------
const navOf = {inventory: () => viewMode === 'trash' ? 'nav-trash' : 'nav-files', browser: () => 'nav-browser', evidence: () => 'nav-evidence'};
let section = 'inventory';
/** Show one section and mark its sidebar entry. Rust is told separately whether the
 * native browser view should be visible. */
function showSection(name) {
  section = name;
  for (const el of document.querySelectorAll('main > section')) el.classList.toggle('active', el.id === name);
  const active = navOf[name]();
  for (const el of document.querySelectorAll('.nav-item')) el.classList.toggle('active', el.id === active);
}
/** Files and Trash share the table. Switching clears selection and view-specific filters
 * so a selection made in one context cannot become an action in the other. */
function switchView(mode) {
  if (mode !== viewMode) {
    viewMode = mode;
    selected.clear();
    selectionAnchor = null;
    tablePage = 0;
    $('findfiles').value = '';
    $('folderfilter').value = '';
  }
  showSection('inventory');
  send('inventory');
  renderFiles();
  $('table-wrap').scrollTop = 0;
  if (mode === 'trash' && !stores.trash.complete && !stores.trash.busy) send('load_trash');
}
$('nav-files').onclick = () => switchView('files');
$('nav-trash').onclick = () => switchView('trash');
$('nav-browser').onclick = () => { showSection('browser'); send('browser'); };
$('nav-evidence').onclick = () => { showSection('evidence'); send('evidence'); };
for (const button of document.querySelectorAll('[data-command]')) button.onclick = () => send(button.dataset.command);
/** Rust returns the shell to the file list when browsing is paused for queued changes. */
window.showFiles = () => showSection('inventory');
/** Rust opened a file in ChatGPT and already made the browser view visible. */
window.selectBrowser = () => showSection('browser');

// The native ChatGPT view is positioned over this element. Report its rectangle whenever
// the layout changes; sections stay laid out while hidden, so it is always measurable.
const browserHost = $('browser-host');
function reportBrowserBounds() {
  const r = browserHost.getBoundingClientRect();
  if (r.width > 0 && r.height > 0) send('bounds:' + JSON.stringify([r.left, r.top, r.width, r.height]));
}
new ResizeObserver(reportBrowserBounds).observe(browserHost);

// ---- Table interaction ------------------------------------------------------
/** Start a listing for the current view. Rust decides whether it may run. */
function startScan() {
  selected.clear();
  selectionAnchor = null;
  tablePage = 0;
  send(viewMode === 'trash' ? 'load_trash' : 'scan');
}
/** Toggle direction for the current column and return to page one without losing selection. */
function sortBy(key) {
  sortDirection = sortKey === key ? -sortDirection : (key === 'name' ? 1 : -1);
  sortKey = key;
  tablePage = 0;
  renderFiles();
}
function changePage(step) {
  tablePage += step;
  renderFiles();
  $('table-wrap').scrollTop = 0;
}
$('scanall').onclick = startScan;
$('empty-action').onclick = () => {
  if (!stores[viewMode].rows.size) { startScan(); return; }
  $('findfiles').value = '';
  $('typefilter').value = 'all';
  $('folderfilter').value = '';
  tablePage = 0;
  renderFiles();
};
$('cancelscan').onclick = () => send('cancel');
$('exportcsv').onclick = () => send('export_csv');
$('prevpage').onclick = () => changePage(-1);
$('nextpage').onclick = () => changePage(1);
$('clearselection').onclick = () => { selected.clear(); selectionAnchor = null; paintSelection(); };
for (const th of document.querySelectorAll('th[data-sort]')) th.querySelector('button').onclick = () => sortBy(th.dataset.sort);
for (const id of ['findfiles', 'typefilter', 'folderfilter']) $(id).addEventListener('input', () => { tablePage = 0; renderFiles(); });
/** Header selection spans all matching pages. A blocked file cannot be reselected until refresh. */
$('selectfiltered').onchange = e => {
  selectionAnchor = null;
  for (const f of filteredFiles()) {
    if (f.action_blocked) continue;
    if (e.target.checked) selected.add(f.id); else selected.delete(f.id);
  }
  paintSelection();
};
/** One listener for every row. A checkbox applies its own state; a file name opens the
 * ChatGPT preview; anything else on the row (and any Shift-click) selects, so ranges can
 * start and end on a name or on empty row space. Trash names select instead of opening. */
$('filebody').addEventListener('click', e => {
  const tr = e.target.closest('tr[data-id]');
  if (!tr) return;
  const id = tr.dataset.id, check = tr.cells[0].firstChild;
  if (e.target === check) {
    selectRange(filteredFiles(), id, check.checked, e.shiftKey);
  } else if (e.target.closest('.file-link') && viewMode === 'files' && !e.shiftKey) {
    send('open:' + id);
    return;
  } else if (e.shiftKey) {
    selectRange(filteredFiles(), id, true, true);
  } else {
    selectRange(filteredFiles(), id, !selected.has(id), false);
  }
  paintSelection();
});
/** File-manager keys: Ctrl+F search, Ctrl+A select all matching, Delete opens the review
 * dialog (nothing changes without confirming there), Escape clears search or selection. */
document.addEventListener('keydown', e => {
  if (section !== 'inventory' || $('review-dialog').open) return;
  const search = $('findfiles'), typing = ['INPUT', 'SELECT'].includes(e.target.tagName) && e.target.type !== 'checkbox';
  const command = e.ctrlKey || e.metaKey, key = e.key.toLowerCase();
  if (command && key === 'f') {
    e.preventDefault();
    search.focus();
    search.select();
  } else if (e.key === 'Escape' && e.target === search && search.value) {
    search.value = '';
    tablePage = 0;
    renderFiles();
  } else if (typing) {
    return;
  } else if (command && key === 'a') {
    e.preventDefault();
    selectionAnchor = null;
    for (const f of filteredFiles()) if (!f.action_blocked) selected.add(f.id);
    paintSelection();
  } else if (e.key === 'Delete' && !$('review-selection').disabled) {
    $('review-selection').click();
  } else if (e.key === 'Escape' && selected.size) {
    selected.clear();
    selectionAnchor = null;
    paintSelection();
  }
});

// ---- Reviewed changes -------------------------------------------------------
/** Request a review of IDs still present in the current view. Rust repeats all safety checks. */
$('review-selection').onclick = () => {
  const rows = stores[viewMode].rows;
  const ids = [...selected].filter(id => rows.has(id) && !reservedIds.has(id));
  send('review:' + JSON.stringify({action: viewMode === 'trash' ? 'restore' : 'trash', ids}));
};
/** Consume the review nonce exactly once on confirmation; cancel and Escape invalidate it.
 * The nonce identifies the reviewed selection, not an arbitrary client-provided file list. */
let reviewNonce = null;
const dialog = $('review-dialog');
dialog.addEventListener('cancel', () => { send('cancel_review'); reviewNonce = null; });
$('review-cancel').onclick = () => { send('cancel_review'); reviewNonce = null; dialog.close(); };
$('review-confirm').onclick = () => {
  if (reviewNonce == null) return;
  const nonce = reviewNonce;
  reviewNonce = null;
  dialog.close();
  send('confirm:' + nonce);
};
/** Show the native-approved selection using textContent. Default focus stays on Cancel. */
window.showReview = r => {
  reviewNonce = r.nonce;
  const trash = r.action === 'trash', count = r.files.length, noun = count === 1 ? 'file' : 'files';
  const total = r.files.reduce((sum, f) => sum + (f.bytes || 0), 0);
  $('review-title').textContent = `${trash ? 'Move' : 'Restore'} ${count.toLocaleString()} ${noun}${trash ? ' to Trash' : ''}?`;
  $('review-total').textContent = `${bytesText(total)} in total` + (jobActive ? ' · runs after the batch in progress' : '');
  $('review-note').textContent = trash
    ? 'ChatGPT removes trashed files permanently after 30 days. Until then they can be restored from Trash. This app never deletes files permanently.'
    : 'These files return to your Library. ChatGPT may reject a restore if your storage is full.';
  const list = document.createDocumentFragment();
  for (const f of r.files) {
    const row = document.createElement('div'), name = document.createElement('span'), size = document.createElement('span');
    row.className = 'review-item';
    name.textContent = f.name;
    size.textContent = bytesText(f.bytes);
    row.append(name, size);
    list.append(row);
  }
  $('review-files').replaceChildren(list);
  const confirm = $('review-confirm');
  confirm.textContent = jobActive ? 'Add to queue' : trash ? 'Move to Trash' : 'Restore';
  confirm.className = trash ? 'btn danger' : 'btn primary';
  dialog.showModal();
  $('review-cancel').focus();
};

// ---- Batch progress ---------------------------------------------------------
/** Rust sends each outcome once, with running totals. Outcomes accumulate here, and the
 * list stays collapsed unless opened; receipt messages are plain text, not markup. */
let job = null, jobShown = 0;
window.showJob = data => {
  jobActive = data.active;
  if (!job || job.id !== data.id || data.offset === 0) {
    $('job-list').replaceChildren();
    jobShown = 0;
  }
  job = data;
  let released = false;
  const items = document.createDocumentFragment();
  for (const o of data.outcomes) {
    const row = document.createElement('div'), name = document.createElement('span'), message = document.createElement('span');
    row.className = ['verified', 'skipped'].includes(o.status) ? 'job-item' : 'job-item problem';
    name.textContent = o.name;
    message.textContent = o.message;
    row.append(name, message);
    items.append(row);
    // A file with an outcome is no longer reserved: failed or skipped rows return to the list.
    released = reservedIds.delete(o.id) || released;
  }
  jobShown += data.outcomes.length;
  $('job-list').append(items);
  $('job').hidden = false;
  if (released) {
    reservedVersion++;
    requestRender();
  } else {
    paintJob();
    updateActions();
  }
};
function paintJob() {
  if (!job) return;
  const trash = job.action === 'trash', done = job.verified + job.attention + job.skipped, total = done + job.remaining;
  const parts = [];
  if (job.active) parts.push(job.stopping ? 'Stopping after the current file…' : `${trash ? 'Moving to Trash' : 'Restoring'} · ${done.toLocaleString()} of ${total.toLocaleString()}`);
  else parts.push(`${job.verified.toLocaleString()} ${job.verified === 1 ? 'file' : 'files'} ${trash ? 'moved to Trash' : 'restored'}`);
  if (job.attention) parts.push(`${job.attention.toLocaleString()} ${job.attention === 1 ? 'needs' : 'need'} attention`);
  if (job.skipped) parts.push(`${job.skipped.toLocaleString()} not attempted`);
  if (queuedBatchCount) parts.push(`${queuedBatchCount} more ${queuedBatchCount === 1 ? 'batch' : 'batches'} queued (${queuedFileCount.toLocaleString()} files)`);
  $('job-text').textContent = parts.join(' · ');
  $('job-bar').style.width = (total ? done / total * 100 : 100) + '%';
  $('job').className = 'job ' + (job.attention ? 'attention' : job.active ? 'running' : 'done');
  $('job-stop').hidden = !job.active;
  $('job-stop').disabled = job.stopping;
  $('job-dismiss').hidden = job.active;
  $('job-toggle').hidden = !jobShown;
}
$('job-stop').onclick = () => send('stop_actions');
$('job-toggle').onclick = () => {
  const open = $('job-list').hidden;
  $('job-list').hidden = !open;
  $('job-toggle').setAttribute('aria-expanded', String(open));
};
$('job-dismiss').onclick = () => { $('job').hidden = true; };

/** Close a stale review and clear selection when credentials or account context change. */
window.invalidateContext = () => {
  reservedIds = new Set();
  reservedVersion++;
  queuedBatchCount = 0;
  queuedFileCount = 0;
  selected.clear();
  selectionAnchor = null;
  reviewNonce = null;
  dialog.close();
  tablePage = 0;
};

// ---- Sidebar status, storage, diagnostics -----------------------------------
/** The sidebar status line is visible in every section, including over the ChatGPT view.
 * Refusals and failures arrive flagged so they stand out from routine progress. */
window.showStatus = (text, alert = false) => {
  $('status').textContent = text;
  $('status').title = text;
  $('status').classList.toggle('alert', alert);
};
/** Quota figures from the storage endpoint; hidden when the shape is not recognized. */
window.showUsage = usage => {
  const used = usage?.used_bytes, allowed = usage?.allowed_bytes;
  const known = Number.isFinite(used) && Number.isFinite(allowed) && allowed > 0;
  $('usage').hidden = !known;
  if (!known) return;
  const share = used / allowed, full = usage.is_over_limit === true || share >= 1;
  $('usage').classList.toggle('full', full);
  $('usage-percent').textContent = full ? 'Full' : Math.floor(share * 100) + '%';
  $('usage-bar').style.width = Math.min(100, share * 100) + '%';
  $('usage-text').textContent = `${bytesText(used)} of ${bytesText(allowed)} used`;
};
const evidenceHelp = [
  'Reports contain field names, types, and array counts.',
  'Response values, request headers, and query strings are excluded. Unknown schema keys are redacted.',
  'Only matching ChatGPT metadata paths are inspected. Empty results do not mean your Library is empty.',
  'Saved reports go to the private reports folder under %LOCALAPPDATA%\\GPTStorageBrowser.',
].join('\n\n');
window.showEvidence = items => {
  $('count').textContent = items.length ? String(items.length) : '';
  $('save').disabled = !items.length;
  const list = $('list');
  list.replaceChildren();
  if (!items.length) {
    const hint = document.createElement('p');
    hint.textContent = 'No responses observed yet. Turn on inspection, then browse your Library in the ChatGPT view.';
    list.append(hint);
    $('detail').textContent = evidenceHelp;
  }
  for (const item of items) {
    const entry = document.createElement('button');
    entry.type = 'button';
    entry.textContent = `${item.status} ${item.method} ${item.path}`;
    entry.onclick = () => {
      for (const other of list.children) other.classList.toggle('active', other === entry);
      $('detail').textContent = item.schema;
    };
    list.append(entry);
  }
};
$('capture').onchange = e => send(e.target.checked ? 'capture:on' : 'capture:off');
$('save').onclick = () => send('export');
$('clearevidence').onclick = () => send('clear');
window.showEvidence([]);
renderFiles();
