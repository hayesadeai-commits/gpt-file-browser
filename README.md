# GPT File Browser

A Windows desktop file manager for your ChatGPT Library, built in Rust. No Chrome extension or .NET runtime is required. It requires the Microsoft WebView2 Runtime (version 123 or newer; the Evergreen runtime on Windows 10/11 already is) on the computer running the app.

## Open and use

Double-click **GPT File Browser.exe** in a local checkout if a release executable has been placed there, or run `gpt-file-browser.exe` from your installation folder. The executable is excluded from Git. Every copy uses the same private data folder, `%LOCALAPPDATA%\GPTStorageBrowser`; moving the executable does not move the saved session.

1. Click **Scan all files**. If sign-in is needed, open **Browser** in the sidebar, complete sign-in and 2FA yourself, then scan again. The app retains your sign-in across launches. **Stop** replaces the button while a scan runs; afterwards it becomes **Refresh**.
2. Click **Name**, **Modified**, or **Size** to sort the entire collection. Results are displayed 100 at a time, but sorting and search cover every indexed file.
3. Search file names (Ctrl+F) and filter by type or folder. Folder filters include descendants, and nested folders are listed by path. Active file names open the corresponding ChatGPT preview. Restore a trashed file before opening it.
4. Click a row or its checkbox to select it, then Shift-click another row, checkbox, or file name to select a range in the current sorted/filtered list, including across pages. The header checkbox (or Ctrl+A) selects **all matching files across pages**; Esc clears the selection. The footer shows the selected count and total size.
5. Choose **Move to Trash…** (or press Delete), inspect the complete selection, and confirm. Nothing changes before that confirmation. Confirmed files are immediately hidden and unselected. While they process, select more files and choose **Add to Trash queue…**. Batches run in confirmation order, one file at a time with readback verification. **Stop** in the progress strip finishes the current file check and cancels the rest; **Details** lists each result.
6. Open **Trash**, select files, and choose **Restore…**. Connect to Library first if opening Trash immediately after launch.
7. **Export CSV** writes the current inventory and a scan summary to `%LOCALAPPDATA%\GPTStorageBrowser\reports`. For file contents, open the ChatGPT preview and use its Download control; the app asks where to save the file.

The sidebar shows how much of your ChatGPT storage is used once a scan has connected, and a status line that stays visible in every section. Refusals and failures appear there in the warning color.

## Features and boundaries

- Complete cursor pagination and recursive Library-folder traversal, including hidden files.
- Fast listing: up to six folders are read at the same time (each folder's own pages stay in order), and the scan asks for larger pages than ChatGPT's default of about 20 items. ChatGPT's own client never requests a page size, so this is an optimization the service may refuse: a page that fails at an unconfirmed size restarts the scan one size smaller (100, then 50, then the service default), ending at exactly the request ChatGPT itself makes. Throttling and gateway errors (HTTP 429/502/503/504) are retried briefly instead of failing the scan. The completion message reports pages and seconds.
- Files and images in ChatGPT storage; connected-provider roots such as Google Drive are excluded.
- Native-style file-browser layout that follows the Windows light/dark setting: sidebar navigation with file counts and a storage meter, an edge-to-edge file table, and a batch progress strip whose details stay collapsed until opened. Attention counts stay visible.
- Numeric size/date/name sorting, search, type/folder filters, cross-page multiselection and size totals.
- Small image thumbnails beside file names, and a type glyph for other files. Hover over a thumbnail for an enlarged image pop-out; moving away dismisses it without navigation. Clicking the file name still opens ChatGPT.
- The app-managed thumbnail and hover caches stay in memory; normal WebView2 browser data is kept in the private profile folder. Thumbnails fall back to original content when a thumbnail fails. Original-content previews fetch up to 32 MiB, fit the window, and cap decoded output at 2048 pixels per side; four enlarged previews are cached. Unsupported or oversized images show an unavailable message.
- Reviewed, queued bulk Trash and restore for owned ordinary Library files. Manage Pages, Sites, folders and shared items in ChatGPT's native interface.
- Fast confirmation of each move: once a file's own metadata no longer answers, the app asks the Trash listing for that one file (by ID, then by name) instead of reading all of Trash, which costs one request per 50 trashed files. Whether ChatGPT honours those filters is learned on the first file and remembered for the rest of the session; if it does not, the complete Trash listing is read instead. A lookup result only counts if the Trash entry was stamped by that request.
- Per-file verification and action receipts. An uncertain or failed result stops the remaining files in that batch and cancels waiting batches. Unprocessed rows return to the list; the affected file remains blocked until refresh. No permanent-delete command.

Refresh if the collection changed elsewhere. Before changes, the app checks identity, ownership, name, parent, size, available timestamp and current Trash state. Failed scans remain visibly incomplete; bulk changes require an initial completed scan. Verified changes update the inventory, so another batch does not require rescanning. A failed or uncertain action blocks only that file until refresh; unaffected files remain available for a new reviewed batch. Queued work is not automatically resumed after closing the app or changing sign-in context. ChatGPT navigation and scanning are paused during processing so they cannot interrupt an active request. While files are being listed, the **ChatGPT**, **Library**, and **Reload** buttons and opening a file are paused for the same reason; stop the scan to browse. Quota usage can update separately from file visibility.

## Local data and connection

The app stores its profiles and generated reports under `%LOCALAPPDATA%\GPTStorageBrowser`, independently of the executable and checkout locations:

- `app-data/chatgpt-profile/`: authenticated ChatGPT browser profile.
- `app-data/interface/`: local UI WebView profile.
- `reports/`: exported metadata, diagnostic reports, and action receipts.

These files may contain credentials, filenames, and account identifiers. Keep them private. Chrome's profile is not imported or modified. If `LOCALAPPDATA` is missing or relative, startup fails instead of falling back to the project directory. Files saved through ChatGPT's native Download dialog go to the destination you choose.

The app learns the Library request context from the signed-in page. Session headers remain in memory and are sent only to ChatGPT. A changed sign-in context invalidates completed scans and pending reviews. Credentials are excluded from CSV, action receipts and diagnostics.

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
| [src/service.rs](src/service.rs) | Authenticated browser requests (bounded retry, slim listing rows) and asynchronous JSON results |
| [src/capture.rs](src/capture.rs) / [src/schema.rs](src/schema.rs) | Session observation, bounded schema diagnostics, and URL boundaries |
| [src/changes.rs](src/changes.rs) | Reviewed soft-delete/restore: batches, eligibility, lookup hints, and script assembly |
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
- A scan is complete only when its queue is empty and every issued page has been accepted. Storage usage is fetched alongside the listing and never gates or invalidates it.
- Rust sends the shell full snapshots only at the start and end of a listing and on sign-in changes. Listing pages and per-file results are sent as incremental patches (`patchView`, `showJob`), so their cost does not grow with the collection.
- Lookup hints (`TrashLookup`) only decide which confirmation read is tried first. They never count as evidence: with every shortcut claimed to work, a change that cannot be seen is still reported as uncertain.
- Request data is inserted into [src/changes.js](src/changes.js) last and exactly once, where a JSON value is expected, so a file name cannot alter the script.
- Only one mutation is active across all batches. Confirmed files are reserved until processed, preventing duplicate queueing. The active operation nonce is independent from newer review-dialog nonces. Persist pending work before issuing it; verify remote state afterward. Retry readback, never an ambiguous mutation.
- An uncertain file is quarantined until refresh, while unrelated files remain usable. A changed session clears account-bound snapshots and reviews.
- UI pagination changes rendering only. Sorting, search, select-all, and Shift-click operate on the complete filtered snapshot, which is cached until its inputs change.
- The shell reports the browser pane's rectangle to Rust (`bounds:`), so the native ChatGPT view follows the layout instead of duplicating its geometry.
- Preview epochs discard stale images, but native outstanding-request counts remain until callbacks finish. Cache resets do not cancel network operations.
- Keep real account values out of comments, fixtures, source documentation, and commits. Runtime receipts are private artifacts.

Module/function comments explain these rules near their implementations. When changing embedded browser scripts, run both Rust and Node checks: the Node harness executes extracted production script text rather than a separately maintained copy. UI logic it runs is delimited by `// #region` markers in the two script files; keep those regions self-contained.
