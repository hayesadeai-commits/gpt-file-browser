# Validation

Documentation reviewed against the current source on November 15, 2026. This file contains no account inventories, personal filenames, file IDs, or machine-specific paths.

## Latest recorded checks

These were run on the revision that added concurrent listing, incremental UI updates, and the redesigned interface:

- 33 Rust unit tests passed.
- JavaScript regression groups passed: preflight comparisons, Trash verification and pagination, the listing fetch (bounded retry, backoff, slim rows), hover lifecycle/retry behavior, Shift-click ranges, queue visibility with whole-collection filtering/sorting/caching, size and file-kind formatting, and JPG content fallback.
- JavaScript syntax checks, `cargo fmt --check`, strict Clippy, and the release build (with path remapping) passed.
- Startup smoke testing passed on the release build. Comparing the checkout before and after startup found no newly created files there.
- The interface was reviewed in a Chromium preview that loads the production markup, stylesheet, and scripts with a mock native bridge and synthetic file data. States covered: empty, scanning, listed, selection, review dialogs for Trash and restore, running/finished/failed batches, Trash, the browser toolbar, and diagnostics, in light and dark themes at 1280x720 and at the 980x680 minimum window size.

Not repeated for this revision:

- **No live scan was run.** The larger listing page sizes and concurrent folder reads have not been observed against the real service. Their fallback (step down to the service default, which is the request ChatGPT's own client sends) is covered by unit tests only. The scan-complete status line reports pages and seconds so the effect can be seen on a real Library.
- No live Trash or restore was performed. The mutation script and its sequencing are unchanged; only how results are published to the interface changed.
- The privacy audit of local Git objects and the release executable described below was performed on an earlier revision and has not been repeated.

These results describe the tested revision. Re-run checks after changes; they are not a guarantee about future service responses or later additions to the repository.

## Automated coverage

Rust unit tests cover:

- Complete pagination and folder traversal, duplicate handling, explicit page limits (including requests still in flight), and missing/repeated cursors.
- Overlapping folder requests: a scan completes only after every issued page is accepted, and stopping a scan makes outstanding replies inert.
- Page-size fallback: stepping down on refusals and unanswered requests, keeping a size once a scan completes with it, and restoring the fastest size when even the service default fails.
- Classifying listing replies (rejected sign-in, refusal, no answer) before they reach the inventory, and equivalence of slim and full listing rows.
- Rebuilding listing URLs without inherited search/type/size filters and rejecting unrelated hosts.
- Mutation eligibility, blocked-file isolation, stopping a sequential batch, FIFO batch promotion, duplicate reservations, and queue limits.
- Account-change invalidation of pending Trash requests and cached rows.
- Diagnostic schema redaction, bounded structural output, and CSV formula protection.
- Validation of the browser-pane rectangle and of IDs placed into ChatGPT URLs.
- Assembly of the shell document: no unresolved placeholders, the content security policy, every native callback, and no remote resources.
- Icon dimensions, transparency, near-full-canvas opaque bounds, and the private runtime directory resolver.

The [Node regression harness](tests/preflight.cjs) executes production script text with mocked browser/service objects. It covers:

- Legacy null timestamps and changed identity, ownership, filename, size, folder, or content version.
- Missing normal-node lookup after trashing, delayed Trash visibility, and issuing only one mutation.
- Restoring a file beyond the first 5,000 Trash entries; rejecting missing/repeated cursors and exhausted pagination limits.
- The listing fetch: retrying only throttling/gateway statuses with backoff and a retry cap, reducing items to the parsed fields without inventing a cursor, passing unexpected shapes and non-JSON error pages through with their status, and the response size limit.
- Hover dismissal, late responses, cached re-entry, scroll dismissal, bounded retries, and dismissal when the table is rebuilt.
- Shift-click ranges across pages, deselection, blocked rows, and missing anchors.
- Original JPG fallback after thumbnail HTTP, network, or decode failure.
- Confirmed queue IDs are hidden and unselected immediately; releasing reservations restores unprocessed rows without reselecting them.
- Whole-collection filtering and sorting: natural name order, unknown values last, case-insensitive search, type filter, folder descendants with cyclic parents, the separate Trash store, and reuse of the sorted result until its inputs change.

The image/DOM mocks test control flow, not native image codecs, real pointer geometry, or every visual layout. Unit tests do not contact ChatGPT or change cloud files.

## Manual and integration evidence

The following manual checks were made on earlier revisions and have not been repeated since the listing and interface changes: sign-in and persistence, complete inventory browsing, sorting/filtering, native preview/download cancellation, and a disposable-file Trash/restore round trip. An existing JPG was decoded through the real WebView loader. Hover event behavior is covered by the mocked regression harness rather than claimed as a complete visual test.

After moving runtime data to `%LOCALAPPDATA%\GPTStorageBrowser`, migration checks verified profile-file counts and lengths. Startup succeeded using that directory and did not create session/report files in the checkout. That startup test alone does not prove that a saved session remains authenticated.

The GPT File Browser executable's Windows icon resource contains six sizes. The redesigned smiling folder icon was rendered and visually checked at 256 pixels; its silhouette reaches nearly to the canvas edges. Windows FileDescription, ProductName, and OriginalFilename fields were checked for the new name. Build-path remapping was checked in an earlier release executable for the tested local identifiers and checkout paths.

Live service behavior can change. Do not claim a real bulk-mutation test from mocked results; use an explicitly authorized disposable file for any future live mutation validation.

## Reproduce the checks

Use the environment setup and commands in [README.md](README.md#build-and-tests). They keep Cargo output outside the source checkout and remap personal build paths. Node.js is required for the JavaScript checks; no npm dependencies are needed.

The `--smoke-test` option creates both WebViews and checks local interface readiness before exiting. It uses the normal private profile and does not verify authentication, inventory completeness, or cloud mutations.

## Runtime storage and sharing

Profiles, generated reports, and action receipts use `%LOCALAPPDATA%\GPTStorageBrowser`. The resolver rejects missing/relative `LOCALAPPDATA` instead of falling back to the executable directory. The private data directory is intentionally persistent and must not be shared.

Keep browser profiles, exports, action receipts, live account diagnostics, shortcuts, and generated builds out of Git. Ignore rules protect future staging but do not remove information already committed; re-audit current files, staged content, history, and release binaries before publishing.

Queued multi-batch behavior is covered by unit tests and mocked UI execution. No live cloud deletion was performed to validate this feature.
