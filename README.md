# GPT File Browser

An unofficial Windows desktop file manager for your ChatGPT Library and storage. Search the full indexed collection, sort by file size or modified date, preview images, and queue bulk moves to Trash or restores. Built in Rust with Microsoft WebView2. No Chrome extension or .NET runtime is required. It requires the Microsoft WebView2 Runtime (version 123 or newer; the Evergreen runtime on Windows 10/11 already is) on the computer running the app.

Download the portable Windows x64 app from [GitHub Releases](https://github.com/hayesadeai-commits/gpt-file-browser/releases/latest). Extract the ZIP and open **GPT File Browser.exe**. The ZIP includes the MIT license and third-party notices. No sign-in data is included; sign in to your own ChatGPT account on first use.

This is an independent project, not affiliated with or endorsed by OpenAI. It uses your signed-in ChatGPT browser session, not an OpenAI API key. Service changes may require updates; see [validation coverage and limitations](VALIDATION.md).

## Open and use

Double-click **GPT File Browser.exe** in a local checkout if a release executable has been placed there, or run `gpt-file-browser.exe` from your installation folder. The executable is excluded from Git. Every copy uses the same private data folder, `%LOCALAPPDATA%\GPTStorageBrowser`; moving the executable does not move the saved session.

1. Click **Scan all files**. If sign-in is needed, open **Browser** in the sidebar, complete sign-in and 2FA yourself, then scan again. The app retains your sign-in across launches. **Stop** replaces the button while a scan runs; afterwards it becomes **Refresh**.
2. Click **Name**, **Modified**, or **Size** to sort the entire collection. Results are displayed 100 at a time, but sorting and search cover every indexed file.
3. Search file names (Ctrl+F) and filter by type or folder. Folder filters include descendants, and nested folders are listed by path. Active file names open the corresponding ChatGPT preview. Restore a trashed file before opening it.
4. Click a row or its checkbox to select it, then Shift-click another row, checkbox, or file name to select a range in the current sorted/filtered list, including across pages. The header checkbox (or Ctrl+A) selects **all matching files across pages**; Esc clears the selection. The footer shows the selected count and total size.
5. Choose **Move to Trash…** (or press Delete), inspect the complete selection, and confirm. Nothing changes before that confirmation. Confirmed files are immediately hidden and unselected. While they process, select more files and choose **Add to Trash queue…**. Batches run in confirmation order. Trash processing runs up to four deletion requests and four verifications simultaneously. Pending confirmations do not pause new requests. Each file gets a fresh preflight, one delete request, and verified readback. Progress distinguishes requests sent from files verified in Trash. Restores remain one at a time. **Stop** in the progress strip stops new work and waits for already-started file checks to finish; **Details** lists each result.
6. Open **Trash**, select files, and choose **Restore…**. Connect to Library first if opening Trash immediately after launch.
7. **Export CSV** writes the current inventory and a scan summary to `%LOCALAPPDATA%\GPTStorageBrowser\reports`. For file contents, open the ChatGPT preview and use its Download control; the app asks where to save the file.

The sidebar shows how much of your ChatGPT storage is used once a scan has connected, and a status line that stays visible in every section. Refusals and failures appear there in the warning color.

## Features and boundaries

- Complete cursor pagination and recursive Library-folder traversal, including hidden files.
- Fast listing: the global file listing reads 50 files per page through `POST /backend-api/files/library`, while a folder-only nodes traversal builds the folder tree. These run together, with up to six outstanding reads. Completion requires both cursor chains and all discovered folders to finish. If the fast endpoint fails, ignores the requested scope, or repeats files, the partial result is discarded and the app restarts the compatible full nodes traversal. After a refusal or a wrong answer it keeps using that traversal until the app is restarted; after a request that merely went unanswered, the next scan tries the fast listing again. That fallback can step down its requested page size (100, 50, then no limit parameter). Throttling and gateway errors are retried briefly. The completion message reports the file count and seconds.
- Files and images in ChatGPT storage; connected-provider roots such as Google Drive are excluded.
- Native-style file-browser layout that follows the Windows light/dark setting: sidebar navigation with file counts and a storage meter, an edge-to-edge file table, and a batch progress strip whose details stay collapsed until opened. Attention counts stay visible.
- Numeric size/date/name sorting, search, type/folder filters, cross-page multiselection and size totals.
- Small image thumbnails beside file names, and a type glyph for other files. Hover over a thumbnail for an enlarged image pop-out; moving away dismisses it without navigation. Clicking the file name still opens ChatGPT.
- The app-managed thumbnail and hover caches stay in memory; normal WebView2 browser data is kept in the private profile folder. Thumbnails fall back to original content when a thumbnail fails. Original-content previews fetch up to 32 MiB, fit the window, and cap decoded output at 2048 pixels per side; four enlarged previews are cached. Unsupported or oversized images show an unavailable message.
- Reviewed, queued bulk Trash and restore for owned ordinary Library files. Manage Pages, Sites, folders and shared items in ChatGPT's native interface.
- Fast confirmation of each move: once a file's own metadata no longer answers, the app asks the Trash listing for that one file (by ID, then by name) instead of reading all of Trash, which costs one request per 50 trashed files. Whether ChatGPT honours those filters is learned on the first file and remembered for the rest of the session; if it does not, the complete Trash listing is read instead. A lookup result only counts if the Trash entry was stamped by that request.
- Per-file verification and action receipts. A batch's receipt is written before each group of requests is sent and when the batch ends, and otherwise at most once a second; a result not yet written leaves its file listed as pending. An uncertain or failed result stops unstarted files in that batch and cancels waiting batches once in-flight checks have finished. Already-started operations still complete their verification. Unprocessed rows return to the list; the affected file remains blocked until refresh. No permanent-delete command.

Refresh if the collection changed elsewhere. Before changes, the app checks identity, ownership, name, parent, size, available timestamp and current Trash state. Failed scans remain visibly incomplete; bulk changes require an initial completed scan. Verified changes update the inventory, so another batch does not require rescanning. A failed or uncertain action blocks only that file until refresh; unaffected files remain available for a new reviewed batch. Queued work is not automatically resumed after closing the app or changing sign-in context. ChatGPT navigation and scanning are paused during processing so they cannot interrupt an active request. While files are being listed, the **ChatGPT**, **Library**, and **Reload** buttons and opening a file are paused for the same reason; stop the scan to browse. Quota usage can update separately from file visibility.

## Local data and connection

The app stores its profiles and generated reports under `%LOCALAPPDATA%\GPTStorageBrowser`, independently of the executable and checkout locations:

- `app-data/chatgpt-profile/`: authenticated ChatGPT browser profile.
- `app-data/interface/`: local UI WebView profile.
- `window.json`: last window position, size and maximized state, saved once the window stops moving or resizing and on close. The app keeps restored windows inside the nearest available screen, above the taskbar.
- `reports/`: exported metadata, diagnostic reports, and action receipts.
- `reports/diagnostic.log`: automatic queue, preview and browser diagnostics, limited to 30 days and 10 MiB (older entries may expire sooner at the size cap). Pruned on startup, daily while logging, and at the size limit. Contains event codes, times and aggregate counts, not filenames, file IDs, credentials or session data. This retention limit applies to the diagnostic log, not exported reports or action receipts.

These files may contain credentials, filenames, and account identifiers. Keep them private. Chrome's profile is not imported or modified. If `LOCALAPPDATA` is missing or relative, startup fails instead of falling back to the project directory. Files saved through ChatGPT's native Download dialog go to the destination you choose.

The app learns the Library request context from authenticated requests made by the signed-in page. Session discovery accepts both the current `POST /backend-api/files/library` request and legacy `GET /backend-api/files/library/nodes` requests; scanning uses the global POST listing for files plus the nodes endpoint for folder discovery, with the original full nodes traversal retained as a fallback. Search terms and filters from a POST request are not copied into the scan. Session headers remain in memory and are sent only to ChatGPT. A changed sign-in context invalidates completed scans and pending reviews. Credentials are excluded from CSV, action receipts and diagnostics.

**Diagnostics** provides optional sanitized response schemas. ChatGPT's observed web endpoints are not a supported public personal-storage API, so service changes may require an app update. Native ChatGPT actions remain available in the embedded browser.

Explicit limits: 10,000 listing pages per scan (counting requests still in flight), six concurrent listing requests, 1,000 Trash pages, 5,000 files per reviewed batch, up to 20 waiting batches, and up to 10,000 reserved files across active and waiting work. Large/invalid responses and repeated cursors stop the operation rather than report a complete inventory. File-change verification may search up to 1,000 pages of 50 Trash items when neither the file's own metadata nor a direct Trash lookup can confirm the change.

## Build and tests

Building requires a Rust Windows MSVC toolchain and the Windows SDK resource compiler. Running requires Microsoft WebView2 Runtime. The JavaScript regression checks also require Node.js; they use only built-in modules and need no npm installation. The full-canvas smiling folder icon is generated from [src/icon.rs](src/icon.rs) and embedded by [build.rs](build.rs); no icon downloads or extra build dependencies are needed. Set `RC` to your resource compiler path if SDK discovery fails. Dependencies are pinned in `Cargo.lock`. From this source directory:

```powershell
$env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'GPTStorageBrowser\build-target'
$env:CARGO_ENCODED_RUSTFLAGS = @(
    "--remap-path-prefix=$env:USERPROFILE=/build-user"
    "--remap-path-prefix=$((Get-Location).Path)=/source"
) -join [char]0x1f
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
node --check src/inventory.js
node --check src/thumbnails.js
node --check src/changes.js
node tests/preflight.cjs
cargo build --release --locked
```

To place the launch icon in this folder, close any running copy and run:

```powershell
Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR 'release\gpt-file-browser.exe') -Destination '.\GPT File Browser.exe'
```

The copied executable is ignored by Git. Preserve `%LOCALAPPDATA%\GPTStorageBrowser` to retain sign-in. To check startup:

```powershell
$smoke = Start-Process -FilePath (Join-Path (Get-Location) 'GPT File Browser.exe') -ArgumentList '--smoke-test' -WindowStyle Hidden -Wait -PassThru
if ($smoke.ExitCode -ne 0) { throw 'GPT File Browser startup check failed' }
```

The smoke test creates both WebViews, requires the local interface-ready signal, and exits after approximately five seconds. It uses the normal private data directory; it is not an isolated profile, an authentication test, or a live cloud mutation test.

See [VALIDATION.md](VALIDATION.md) for a privacy-safe validation summary.

## Sharing source safely

Share only this source repository. Never upload `%LOCALAPPDATA%\GPTStorageBrowser`, which contains private browser profiles and action receipts. Keep generated build output outside the checkout using the build command above. Do not include machine-specific shortcuts.

The source ignore rules exclude local secrets, profiles, reports, binaries, and build artifacts. Cargo packaging uses an explicit source allowlist. Ignore rules do not remove previously committed secrets; review staged files and history before publishing. Before publishing, inspect `git status`, `git diff --cached`, and `git remote -v`. Review author/email settings before creating commits, and scan any existing history as well as current files.

## Maintainer source map

| File | Responsibility |
| --- | --- |
| [src/main.rs](src/main.rs) | Native windows, separate WebViews, IPC validation, scan/job state, page-size fallback, and action receipts |
| [src/inventory.rs](src/inventory.rs) | Normalized metadata, concurrent folder traversal, pagination, and completeness |
| [src/service.rs](src/service.rs) / [src/listing_probe.js](src/listing_probe.js) | Authenticated read requests, bounded retry, slim listing rows, and read-only endpoint comparison |
| [src/capture.rs](src/capture.rs) / [src/schema.rs](src/schema.rs) | Session observation, bounded schema diagnostics, and URL boundaries |
| [src/changes.rs](src/changes.rs) | Reviewed soft-delete/restore: batches, eligibility, lookup hints, and script assembly |
| [src/window_state.rs](src/window_state.rs) | Remember window bounds and fit them to the usable screen area |
| [src/diagnostics.rs](src/diagnostics.rs) | Bounded local diagnostic log with age and size retention |
| [src/changes.js](src/changes.js) / [src/preflight.js](src/preflight.js) | The browser-side sequence for one file: live metadata check, one mutation, readback verification |
| [src/ui.html](src/ui.html) / [src/ui.css](src/ui.css) | Trusted local shell markup, icon sprite, and the single stylesheet (light/dark tokens) |
| [src/inventory.js](src/inventory.js) | View stores, incremental updates, sorting/filtering, range selection, review dialogs, and batch progress |
| [src/preview.rs](src/preview.rs) / [src/thumbnails.js](src/thumbnails.js) | Image loading/rasterization, lazy thumbnails, hover state, and caches |
| [src/download.rs](src/download.rs) / [src/export.rs](src/export.rs) | Native save dialog and safe CSV cells |
| [src/icon.rs](src/icon.rs) / [build.rs](build.rs) | Shared icon rasterizer and Windows executable resource |
| [tests/preflight.cjs](tests/preflight.cjs) | Production-script regression tests using mocked browser/service objects |

### State and safety rules

- Only the local shell receives IPC. Authenticated content runs in a separate WebView and profile.
- Every listing request has an ID that is never reused; a reply is applied only while its request is still registered, so replies from a stopped or restarted scan are inert. Trash keeps its own generation. A review nonce binds confirmation to the native-approved selection.
- Files and folders have separate cursor namespaces. A scan is complete only when both listings finish, its queue is empty, and every issued page has been accepted. Storage usage is fetched alongside the listing and never gates or invalidates it.
- Rust sends the shell full snapshots only at the start and end of a listing and on sign-in changes. Listing pages and per-file results are sent as incremental patches (`patchView`, `showJob`), so their cost does not grow with the collection.
- Lookup hints (`TrashLookup`) only decide which confirmation read is tried first. They never count as evidence: with every shortcut claimed to work, a change that cannot be seen is still reported as uncertain.
- Request data is inserted into [src/changes.js](src/changes.js) last and exactly once, where a JSON value is expected, so a file name cannot alter the script.
- Four Trash request slots and four independent verification slots run simultaneously; restores stay sequential. There is no separate confirmation-backlog cap; sending continues through the reviewed batch while verification catches up. Request-sent counts do not imply verified success. The progress strip lists up to 50 files awaiting their check by name and counts the rest. Results are matched by both batch nonce and file ID, so out-of-order replies cannot verify the wrong file. Stop, failure, or sign-in change prevents new work and waits for every in-flight check to finish. Confirmed files are reserved until processed, preventing duplicate queueing. The active operation nonce is independent from newer review-dialog nonces. Persist pending work before issuing it; verify remote state afterward. Retry readback, never an ambiguous mutation.
- An uncertain file is quarantined until refresh, while unrelated files remain usable. A changed session clears account-bound snapshots and reviews.
- UI pagination changes rendering only. Sorting, search, select-all, and Shift-click operate on the complete filtered snapshot, which is cached until its inputs change.
- The shell reports the browser pane's rectangle to Rust (`bounds:`), so the native ChatGPT view follows the layout instead of duplicating its geometry.
- Preview epochs discard stale images, but native outstanding-request counts remain until callbacks finish. Cache resets do not cancel network operations.
- Keep real account values out of comments, fixtures, source documentation, and commits. Runtime receipts are private artifacts.

Module/function comments explain these rules near their implementations. When changing embedded browser scripts, run both Rust and Node checks: the Node harness executes extracted production script text rather than a separately maintained copy. UI logic it runs is delimited by `// #region` markers in the two script files; keep those regions self-contained.

## License and third-party notices

GPT File Browser's original code is licensed under the [MIT License](LICENSE), copyright 2026 hayesadeai-commits. Third-party code retains its own licenses; the MIT grant does not replace them.

[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists the Windows dependency graph and links to preserved license, copyright, and additional-notice texts, including the Microsoft WebView2 SDK loader and the Rust standard library. The separately installed WebView2 Runtime is not bundled with this project.

When sharing an executable, include `LICENSE`, `THIRD_PARTY_NOTICES.md`, and the complete `third-party/` directory alongside it. For a build produced by the commands above:

```powershell
$releaseDir = Join-Path $env:CARGO_TARGET_DIR 'release'
Copy-Item -LiteralPath 'LICENSE', 'THIRD_PARTY_NOTICES.md' -Destination $releaseDir
Copy-Item -LiteralPath 'third-party' -Destination $releaseDir -Recurse -Force
```

After changing dependencies or Rust toolchains, refresh and verify the bundle with Python 3:

```powershell
python scripts/generate_notices.py
python scripts/generate_notices.py --check
```

The generator uses cached Cargo metadata and retrieves the pinned WebView2 license texts from their public upstream sources. It fails on missing notices, changed WebView2 provenance, or obsolete bundle files rather than silently omitting them. The bundled notices target `x86_64-pc-windows-msvc`; review/regenerate them for other targets.

## Feedback and forks

Forks are welcome. This repository does not accept pull requests; pull request creation is disabled. Report reproducible bugs or request features through [GitHub Issues](https://github.com/hayesadeai-commits/gpt-file-browser/issues).

Do not attach browser profiles, authentication headers, account inventories, or private file contents to public issues. Report sensitive security findings through [private vulnerability reporting](https://github.com/hayesadeai-commits/gpt-file-browser/security/advisories/new).

## Read-only endpoint comparison

For maintainers investigating service changes, close the normal app and run `GPT File Browser.exe --compare-listings`. Leave the comparison window open and avoid changing files while it runs. It compares the original complete node traversal with two complete 50-item POST listings and a folder-only traversal, and probes whether a 100-item first page is accepted. The production scanner deliberately uses the fully compared 50-item page size.

Only aggregate counts, timings, and mismatch counts are saved to `%LOCALAPPDATA%\GPTStorageBrowser\reports\listing-comparison.json`. Names, IDs, timestamps, cursor values, and authorization headers remain in memory. Keep the report private; it contains account-level counts. The comparison does not delete, restore, or upload files. A one-page size probe alone does not establish complete coverage at that size.

Diagnostic `queue_state` entries distinguish `sending`, `checking`, and `waiting_for_check`. Their sum is `unfinished`; `confirming` includes both checking and waiting. `sent` is cumulative and does not mean confirmed success; `verified` counts confirmed results. Each entry has a Unix timestamp in seconds and a batch identifier for matching related events.
