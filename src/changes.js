/**
 * Preflight and one mutation (send phase), independent readback (verify phase),
 * or the full sequential sequence for restore. Verification cannot issue a mutation.
 * Runs inside the signed-in ChatGPT page. changes.rs fills in the two placeholders
 * below (the preflight function from preflight.js, then the request data) and awaits
 * the returned promise. Each placeholder must appear exactly once, in code, never in a
 * comment: request data contains file names and is only safe where a JSON value is.
 *
 * Rules that must survive any edit:
 * - Exactly one mutation is sent, and it is never retried. Only reads are repeated.
 * - Permanent deletion is never requested.
 * - "verified" needs evidence read back after the request: the file's own node, or its
 *   entry in the Trash listing. An HTTP success alone is not evidence.
 * - What has been learned about cheaper lookups (the `lookup` hints) only decides which
 *   reads are tried first. It never changes what counts as evidence.
 */
(async () => {
  if (location.origin !== 'https://chatgpt.com') throw Error('Sign-in required');
  const a = __ARGS__, f = a.file, trashing = a.action === 'trash';
  const base = '/backend-api/files/library/files/' + encodeURIComponent(f.id);
  const headers = {Authorization: a.authorization};
  if (a.account) headers['ChatGPT-Account-ID'] = a.account;
  /** One authenticated request with a 30-second limit. A non-JSON body is returned as null. */
  async function req(url, method = 'GET', body) {
    const h = {...headers};
    if (body) h['Content-Type'] = 'application/json';
    const r = await fetch(url, {method, credentials: 'include', headers: h, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(30000)});
    let b = null;
    try { b = await r.json(); } catch {}
    return {status: r.status, body: b};
  }
  const isTrashed = n => n && n.trashed_at != null;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  /** One page of the dedicated Trash listing. 50 is the page size ChatGPT's own client uses. */
  const trashPage = (cursor, filter) => req('/backend-api/files/library', 'POST', {limit: 50, cursor, trashed_only: true, include_hidden_files: true, ...filter});

  // true = works here, false = does not, null = not yet known. Reported back with every
  // result so the next file can skip what is known not to work.
  const lookup = {by_id: a.lookup?.by_id ?? null, by_name: a.lookup?.by_name ?? null, node: a.lookup?.node ?? null};
  let trashProblem = null;

  // Normal node lookup excludes trashed files, so the complete answer is the Trash listing
  // itself. Reading all of it costs one request per 50 trashed files; it is the fallback.
  // The page bound matches the Trash browser; repeated/malformed cursors fail closed.
  async function walkTrash() {
    let cursor = null;
    const seen = new Set();
    for (let page = 0; page < 1000; page++) {
      const r = await trashPage(cursor);
      if (r.status !== 200 || !Array.isArray(r.body?.items) || !Object.hasOwn(r.body, 'cursor') || !(r.body.cursor === null || typeof r.body.cursor === 'string')) {
        trashProblem = 'Trash response could not be verified (HTTP ' + r.status + ').';
        return null;
      }
      const match = r.body.items.find(n => n.id === f.id);
      if (match) return match;
      cursor = r.body.cursor;
      if (!cursor) return null;
      if (seen.has(cursor)) {
        trashProblem = 'Trash repeated a pagination cursor.';
        return null;
      }
      seen.add(cursor);
    }
    trashProblem = 'Trash verification exceeded 1,000 pages. No further changes were attempted.';
    return null;
  }

  // The same listing can be asked for one file, by ID or by name, which is a single request
  // however large Trash is. Whether the service honours these filters for Trash is learned,
  // not assumed: a miss here is never proof of absence, and the full read stays the fallback.
  const QUICK = {by_id: () => ({allowlist_ids: [f.id]}), by_name: () => ({q: f.name})};
  const QUICK_ROUNDS = 4;
  /** Ask Trash for this file with one filter. With `since`, an entry trashed before that
   * time is ignored: only an entry stamped by this request is evidence that it worked. */
  async function quickFind(kind, since) {
    let r;
    try { r = await trashPage(null, QUICK[kind]()); } catch { return null; }
    if (r.status !== 200 || !Array.isArray(r.body?.items)) {
      // The filter itself was refused; anything else (throttling, an outage) says nothing.
      if ([400, 404, 405, 422].includes(r.status)) lookup[kind] = false;
      return null;
    }
    // An ID filter that returns other files was ignored by the service.
    if (kind === 'by_id' && r.body.items.some(n => n?.id !== f.id)) lookup[kind] = false;
    const match = r.body.items.find(n => n?.id === f.id && isTrashed(n));
    if (!match || (since != null && !(Date.parse(match.trashed_at) >= since))) return null;
    return match;
  }
  /** The quick lookups worth trying: the ones proven to work, otherwise the untested ones. */
  function quickKinds() {
    const usable = Object.keys(QUICK).filter(kind => lookup[kind] !== false);
    const proven = usable.filter(kind => lookup[kind] === true);
    return proven.length ? proven : usable;
  }
  /** Try the quick lookups, up to `rounds` times with a short pause between, because the
   * listing can lag behind a request. Returns the entry (or null) and which were tried. */
  async function quickTrash(rounds, since) {
    const tried = new Set();
    for (let round = 0; round < rounds; round++) {
      const kinds = quickKinds();
      if (!kinds.length) break;
      if (round) await pause(600);
      for (const kind of kinds) {
        tried.add(kind);
        const match = await quickFind(kind, since);
        if (match) {
          if (lookup[kind] === null) lookup[kind] = true;
          return {match, tried};
        }
      }
    }
    return {match: null, tried};
  }
  /** The full listing just found the file. A quick lookup that still cannot find it is not
   * usable here, so later files skip it instead of paying for it again. */
  async function settleQuick(tried, since) {
    for (const kind of tried) {
      if (lookup[kind] === false) continue;
      const found = await quickFind(kind, since);
      if (lookup[kind] !== false) lookup[kind] = !!found;
    }
  }
  /** Locate a trashed file before restoring it: one quick request, else the full listing. */
  async function findInTrash() {
    const {match, tried} = await quickTrash(1, null);
    if (match) return match;
    const found = await walkTrash();
    if (found) await settleQuick(tried, null).catch(() => {});
    return found;
  }

  // Sending and verification are separate native queues. Verification never enters
  // the preflight/mutation block, including when the original response was lost.
  let sent = a.sent ?? Date.now();
  let mutation = a.mutation_status == null ? null : {status: a.mutation_status};
  if (a.phase === 'verify' && (!trashing || !Number.isFinite(a.sent))) throw Error('Invalid verification context');
  if (a.phase !== 'verify') {
    // Re-read metadata immediately before mutation; scans may be older than the review dialog.
    // Nothing has been sent yet, so a failure here is a plain failure, not an uncertain state.
    let before;
    try {
      before = trashing ? await req(base + '/node') : {status: 200, body: await findInTrash()};
    } catch {
      return {status: 'failed', message: 'Could not reach ChatGPT to check the file before changing it. Nothing was sent.', lookup};
    }
    if (before.status !== 200 || before.body?.id !== f.id) return {status: 'failed', message: trashProblem || 'Could not verify the selected file before changing it.', lookup};
    const n = before.body;
    const changed = (__PREFLIGHT__)(f, n);
    if (changed) return {status: 'failed', message: 'The file changed since scanning (' + changed + '). Refresh and review it again.', lookup};
    if (trashing && isTrashed(n) || !trashing && !isTrashed(n)) return {status: 'failed', message: 'File state changed since scanning. Refresh before retrying.', lookup};

    // Send exactly one mutation. A timeout can occur after the service commits it.
    sent = Date.now();
    mutation = null;
    try {
      if (trashing) {
        const q = new URLSearchParams({soft_delete: 'true', file_name: f.name});
        if (f.file_id) q.set('file_id', f.file_id);
        if (f.parent) q.set('parent_directory_id', f.parent);
        mutation = await req(base + '?' + q, 'DELETE');
      } else {
        mutation = await req(base + '/restore', 'POST');
      }
    } catch {}

  }
  if (a.phase === 'send') return {status:'sent', sent, mutation_status:mutation?.status ?? null, lookup};

  // Verify state independently of the mutation HTTP code; successful transport is not proof.
  const verified = node => ({status: 'verified', message: trashing ? 'Moved to Trash and verified.' : 'Restored and verified.', node, lookup});
  let after = null;
  /** The file's own node, if it shows the state this request was meant to produce. */
  async function readNode() {
    try { after = await req(base + '/node'); } catch {}
    const shows = after?.status === 200 && after.body?.id === f.id && Object.hasOwn(after.body, 'trashed_at')
      && (trashing ? isTrashed(after.body) : !isTrashed(after.body));
    return shows ? after.body : null;
  }
  if (!trashing) {
    const node = await readNode();
    if (node) return verified(node);
  } else {
    // Quick evidence must be stamped by this request, allowing for clock differences.
    const since = sent - 120000;
    const quickTried = new Set();
    const askNode = async () => {
      const node = await readNode();
      if (node) lookup.node = true;
      return node;
    };
    const askTrash = async () => {
      const {match, tried} = await quickTrash(QUICK_ROUNDS, since);
      for (const kind of tried) quickTried.add(kind);
      return match;
    };
    /** Trash showed the file after its node, already read, did not: ask Trash first next time. */
    const noteNode = () => { if (after) lookup.node = false; };
    // Both reads always happen before giving up, so `after` is available for the diagnosis
    // below. Where node lookups are known not to show trashed files, Trash is asked first.
    for (const ask of lookup.node === false ? [askTrash, askNode] : [askNode, askTrash]) {
      const node = await ask();
      if (node) {
        if (ask === askTrash) noteNode();
        return verified(node);
      }
    }
    // Retry only readback for eventual consistency, never the DELETE/restore request.
    for (let attempt = 0; attempt < 3; attempt++) {
      if (attempt) await pause(750);
      let match = null;
      try { match = await walkTrash(); } catch {}
      if (match && isTrashed(match)) {
        await settleQuick(quickTried, since).catch(() => {});
        noteNode();
        return verified(match);
      }
    }
  }
  if (mutation && mutation.status >= 400 && after?.status === 200 && after.body?.id === f.id && (trashing ? !isTrashed(after.body) : isTrashed(after.body))) {
    return {status: 'failed', message: 'Service rejected the change (HTTP ' + mutation.status + '). File state is unchanged.', lookup};
  }
  // Preserve ambiguity so the coordinator blocks this file instead of risking a duplicate action.
  return {status: 'uncertain', message: 'The request was sent, but its result could not be verified' + (mutation ? ' (HTTP ' + mutation.status + ')' : '') + '. The file may already be in Trash. ' + (trashProblem || '') + ' The batch stopped; refresh Library and Trash before trying again.', lookup};
})()
