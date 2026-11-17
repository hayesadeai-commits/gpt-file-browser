# Validation

Documentation reviewed against the current source on November 16, 2026. This file contains no account inventories, personal filenames, file IDs, or machine-specific paths.

## Latest recorded checks

- 36 Rust unit tests passed.
- JavaScript regression groups passed: preflight comparisons, Trash verification and pagination, the listing fetch (bounded retry, backoff, slim rows), hover lifecycle/retry behavior, Shift-click ranges, queue visibility with whole-collection filtering/sorting/caching, size and file-kind formatting, and JPG content fallback.
- JavaScript syntax checks, `cargo fmt --check`, strict Clippy, and the release build (with path remapping) passed.
- Startup smoke testing passed on the release build. Comparing the checkout before and after startup found no newly created files there.
- The interface was reviewed in a Chromium preview that loads the production markup, stylesheet, and scripts with a mock native bridge and synthetic file data. States covered: empty, scanning, listed, selection, review dialogs for Trash and restore, running/finished/failed batches, Trash, the browser toolbar, and diagnostics, in light and dark themes at 1280x720 and at the 980x680 minimum window size.
- Privacy audit of everything Git holds: every version of every file in every commit, together with commit messages, authors, and dates, was searched for user and machine names, user-profile and absolute paths, email addresses, tokens, keys, cookies, signed URLs, account-style IDs, and IP addresses. The same content was compared with a private inventory export for real file IDs, names, sizes, and timestamps. None are present. Release executables built with the documented path remapping were searched for the user, machine, and checkout names; none were present.

Not verified against the live service in a recorded test:

- The larger listing page sizes and concurrent folder reads. Their fallback (step down to the node-listing service default with no size parameter) is covered by unit tests only. The scan-complete status line reports pages and seconds so the effect can be seen on a real Library.
- The direct Trash lookup. ChatGPT's own client sends the ID filter (`allowlist_ids`) and the name search (`q`) in the same listing request it uses for Trash, but whether they are honoured together with `trashed_only` is unconfirmed. If they are not, the script learns that on the first file and reads the complete Trash listing instead; the mocked harness asserts that it then sends no filtered requests at all.
- Trash and restore with the current confirmation reads. Each file still gets exactly one mutation, in the same sequence.

These results describe the tested revision. Re-run checks after changes; they are not a guarantee about future service responses or later additions to the repository.

## Automated coverage

Rust unit tests cover:

- Complete pagination and folder traversal, duplicate handling, explicit page limits (including requests still in flight), and missing/repeated cursors.
- Overlapping folder requests: a scan completes only after every issued page is accepted, and stopping a scan makes outstanding replies inert.
- Page-size fallback: stepping down on refusals and unanswered requests, keeping a size once a scan completes with it, and restoring the fastest size when even the service default fails.
- Classifying listing replies (rejected sign-in, refusal, no answer) before they reach the inventory, and equivalence of slim and full listing rows.
- Rebuilding listing URLs without inherited search/type/size filters and rejecting unrelated hosts.
- Mutation eligibility, blocked-file isolation, stopping a sequential batch, FIFO batch promotion, duplicate reservations, and queue limits.
- Merging of Trash lookup hints, and assembly of the mutation script: each placeholder appears once, a hostile file name stays inside its JSON string, and no permanent or batch delete is requested.
- Account-change invalidation of pending Trash requests and cached rows.
- Diagnostic schema redaction, bounded structural output, and CSV formula protection.
- Validation of the browser-pane rectangle and of IDs placed into ChatGPT URLs.
- Assembly of the shell document: no unresolved placeholders, the content security policy, every native callback, and no remote resources.
- Icon dimensions, transparency, near-full-canvas opaque bounds, and the private runtime directory resolver.

The [Node regression harness](tests/preflight.cjs) executes production script text with mocked browser/service objects. It covers:

- Legacy null timestamps and changed identity, ownership, filename, size, folder, or content version.
- Missing normal-node lookup after trashing, delayed Trash visibility, and issuing only one mutation.
- Confirming a move with one filtered Trash request regardless of Trash size; learning which filter works; polling a proven lookup while the listing lags; dropping refused or ignored filters; and sending no filtered requests once both are known not to work.
- Rejecting a filtered Trash result stamped before the request, a rejected request reported as a failure, an invisible result reported as uncertain even when every shortcut is claimed to work, and a failed pre-check reported as "nothing was sent".
- Restoring a file beyond the first 5,000 Trash entries; rejecting missing/repeated cursors and exhausted pagination limits.
- The listing fetch: retrying only throttling/gateway statuses with backoff and a retry cap, reducing items to the parsed fields without inventing a cursor, passing unexpected shapes and non-JSON error pages through with their status, and the response size limit.
- Hover dismissal, late responses, cached re-entry, scroll dismissal, bounded retries, and dismissal when the table is rebuilt.
- Shift-click ranges across pages, deselection, blocked rows, and missing anchors.
- Original JPG fallback after thumbnail HTTP, network, or decode failure.
- Confirmed queue IDs are hidden and unselected immediately; releasing reservations restores unprocessed rows without reselecting them.
- Whole-collection filtering and sorting: natural name order, unknown values last, case-insensitive search, type filter, folder descendants with cyclic parents, the separate Trash store, and reuse of the sorted result until its inputs change.

The image/DOM mocks test control flow, not native image codecs, real pointer geometry, or every visual layout. Unit tests do not contact ChatGPT or change cloud files.

## Manual and integration evidence

The following manual checks were made on earlier revisions and have not been repeated for the current listing, interface, and confirmation code: sign-in and persistence, complete inventory browsing, sorting/filtering, native preview/download cancellation, and a disposable-file Trash/restore round trip. An existing JPG was decoded through the real WebView loader. Hover event behavior is covered by the mocked regression harness rather than claimed as a complete visual test.

Startup uses `%LOCALAPPDATA%\GPTStorageBrowser` and does not create session or report files in the checkout. A startup test alone does not prove that a saved session remains authenticated.

The GPT File Browser executable's Windows icon resource contains six sizes. The smiling folder icon was rendered and visually checked at 256 pixels; its silhouette reaches nearly to the canvas edges. Windows FileDescription, ProductName, and OriginalFilename fields were checked for the product name.

Live service behavior can change. Do not claim a real bulk-mutation test from mocked results; use an explicitly authorized disposable file for any future live mutation validation.

## Reproduce the checks

Use the environment setup and commands in [README.md](README.md#build-and-tests). They keep Cargo output outside the source checkout and remap personal build paths. Node.js is required for the JavaScript checks; no npm dependencies are needed.

The `--smoke-test` option creates both WebViews and checks local interface readiness before exiting. It uses the normal private profile and does not verify authentication, inventory completeness, or cloud mutations.

## Runtime storage and sharing

Profiles, generated reports, and action receipts use `%LOCALAPPDATA%\GPTStorageBrowser`. The resolver rejects missing/relative `LOCALAPPDATA` instead of falling back to the executable directory. The private data directory is intentionally persistent and must not be shared.

Keep browser profiles, exports, action receipts, live account diagnostics, shortcuts, and generated builds out of Git. Ignore rules protect future staging but do not remove information already committed; re-audit current files, staged content, history, and release binaries before publishing.

Queued multi-batch behavior is covered by unit tests and mocked UI execution. No live cloud deletion was performed to validate this feature.

## License bundle validation

The root license and Cargo package metadata declare MIT for this project's own code. The third-party notice generator resolves the locked Windows dependency graph, includes build dependencies, and preserves upstream license/copyright/notice texts with SHA-256 digests. Missing WebView2 crate licenses are taken from the exact crate revisions; the native loader notice is taken from its official SDK package license. Rust standard-library notices come from the installed toolchain's copyright bundle.

Run `python scripts/generate_notices.py --check` and `cargo package --list --allow-dirty` to check reproducibility and packaging. Source and executable distributions must keep `LICENSE`, `THIRD_PARTY_NOTICES.md`, and all referenced `third-party/` files. This is scoped to the recorded dependencies, Windows target, and toolchain; updates require a renewed check.

## Reviewed advisory: glib on non-Windows targets

On November 16, 2026, Dependabot alert [#1](https://github.com/hayesadeai-commits/gpt-file-browser/security/dependabot/1) was reviewed for [GHSA-wrw7-89jp-8q8g / RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html), affecting `glib >=0.15.0, <0.20.0`.

`Cargo.lock` includes `glib 0.18.5` through the Linux GTK dependencies of Tao/Wry. The supported Windows target (`x86_64-pc-windows-msvc`) does not resolve or compile this crate. This can be checked without downloading dependencies:

```powershell
cargo tree --locked --offline --target all -i glib
cargo tree --locked --offline --target x86_64-pc-windows-msvc -i glib
```

The first command shows the GTK dependency chain; the Windows command reports nothing to print. This is target exclusion, not a claim that the affected library has been patched. The specific alert is dismissed as **vulnerable code is not used** for the current Windows build. Dependency alerts, secret scanning, push protection, and private vulnerability reporting remain enabled.

Reassess and reopen the alert if Linux support is added, target/dependency configuration changes bring glib into a supported build, or new evidence contradicts this exclusion. A Linux build must address the affected dependency before it is supported or distributed.

## Library session discovery compatibility

The live signed-in Library was observed issuing `POST /backend-api/files/library` without issuing the legacy initial `GET /backend-api/files/library/nodes` request. Requiring only the latter left the scanner waiting for a session until its connection timeout, even though the embedded browser displayed files.

Session discovery now recognizes both request forms on the exact HTTPS ChatGPT origin. The POST request supplies session headers only; its search/filter body is not retained. The existing nodes endpoint remains the scan transport. A full live scan completed successfully with the corrected discovery logic, including folder traversal, without reauthentication or cloud mutations. Personal filenames, IDs, totals, and session values are intentionally omitted here.

A regression test covers current/legacy discovery, pagination-bearing requests, and rejection of unrelated hosts, ports, methods, and mutation paths. Rust tests, strict Clippy, and the JavaScript regressions pass. This successful listing test does not change the separate unverified-live-mutation limitations described above.
