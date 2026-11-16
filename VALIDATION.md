# Validation

Documentation reviewed against the current source on November 15, 2026. This file contains no account inventories, personal filenames, file IDs, or machine-specific paths.

## Latest recorded checks

- 22 Rust unit tests passed.
- JavaScript regression groups passed: preflight comparisons, Trash verification and pagination, hover lifecycle/retry behavior, Shift-click ranges, queue visibility/selection, and JPG content fallback.
- JavaScript syntax checks, strict Clippy, and the release build passed.
- Startup smoke testing passed. Comparing the checkout before and after startup found no newly created files there.
- A privacy audit of the source, local Git objects, and remapped release executable found no tested personal identifiers or checkout paths. The local executable is not tracked by Git.

These results describe the tested revision. Re-run checks after changes; they are not a guarantee about future service responses or later additions to the repository.

## Automated coverage

Rust unit tests cover:

- Complete pagination and folder traversal, duplicate handling, explicit page limits, and missing/repeated cursors.
- Rebuilding listing URLs without inherited search/type filters and rejecting unrelated hosts.
- Mutation eligibility, blocked-file isolation, stopping a sequential batch, FIFO batch promotion, duplicate reservations, and queue limits.
- Account-change invalidation of pending Trash requests and cached rows.
- Diagnostic schema redaction, bounded structural output, and CSV formula protection.
- Icon dimensions, transparency, near-full-canvas opaque bounds, and the private runtime directory resolver.

The [Node regression harness](tests/preflight.cjs) executes production script text with mocked browser/service objects. It covers:

- Legacy null timestamps and changed identity, ownership, filename, size, folder, or content version.
- Missing normal-node lookup after trashing, delayed Trash visibility, and issuing only one mutation.
- Restoring a file beyond the first 5,000 Trash entries; rejecting missing/repeated cursors and exhausted pagination limits.
- Hover dismissal, late responses, cached re-entry, scroll dismissal, and retryable loader saturation.
- Shift-click ranges across pages, deselection, blocked rows, and missing anchors.
- Original JPG fallback after thumbnail HTTP, network, or decode failure.
- Confirmed queue IDs are hidden and unselected immediately; releasing reservations restores unprocessed rows without reselecting them.

The image/DOM mocks test control flow, not native image codecs, real pointer geometry, or every visual layout. Unit tests do not contact ChatGPT or change cloud files.

## Manual and integration evidence

Earlier manual checks exercised sign-in and persistence, complete inventory browsing, sorting/filtering, native preview/download cancellation, and a disposable-file Trash/restore round trip. Inline image previews and the compact table layout were observed; an existing JPG was decoded through the real WebView loader. Hover event behavior is covered by the mocked regression harness rather than claimed as a complete visual test.

After moving runtime data to `%LOCALAPPDATA%\GPTStorageBrowser`, migration checks verified profile-file counts and lengths. Startup succeeded using that directory and did not create session/report files in the checkout. That startup test alone does not prove that a saved session remains authenticated.

The GPT File Browser executable's Windows icon resource contains six sizes. The redesigned smiling folder icon was rendered and visually checked at 256 pixels; its silhouette reaches nearly to the canvas edges. Windows FileDescription, ProductName, and OriginalFilename fields were checked for the new name. Build-path remapping was checked in the release executable for the tested local identifiers and checkout paths.

Live service behavior can change. Do not claim a real bulk-mutation test from mocked results; use an explicitly authorized disposable file for any future live mutation validation.

## Reproduce the checks

Use the environment setup and commands in [README.md](README.md#build-and-tests). They keep Cargo output outside the source checkout and remap personal build paths. Node.js is required for the JavaScript checks; no npm dependencies are needed.

The `--smoke-test` option creates both WebViews and checks local interface readiness before exiting. It uses the normal private profile and does not verify authentication, inventory completeness, or cloud mutations.

## Runtime storage and sharing

Profiles, generated reports, and action receipts use `%LOCALAPPDATA%\GPTStorageBrowser`. The resolver rejects missing/relative `LOCALAPPDATA` instead of falling back to the executable directory. The private data directory is intentionally persistent and must not be shared.

Keep browser profiles, exports, action receipts, live account diagnostics, shortcuts, and generated builds out of Git. Ignore rules protect future staging but do not remove information already committed; re-audit current files, staged content, history, and release binaries before publishing.

Queued multi-batch behavior is covered by unit tests and mocked UI execution. No live cloud deletion was performed to validate this feature.
