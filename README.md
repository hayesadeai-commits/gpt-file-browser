# GPT File Browser

A Windows desktop file manager for your ChatGPT Library, built in Rust. No Chrome extension or .NET runtime is required. It requires the Microsoft WebView2 Runtime on the computer running the app.

## Open and use

Double-click **GPT File Browser.exe** in a local checkout if a release executable has been placed there, or run `gpt-file-browser.exe` from your installation folder. The executable is excluded from Git. Every copy uses the same private data folder, `%LOCALAPPDATA%\GPTStorageBrowser`; moving the executable does not move the saved session. The private folder retains its `GPTStorageBrowser` name for compatibility with existing sessions.

1. Click **Scan all files**. If sign-in is needed, use **Open ChatGPT**, complete sign-in and 2FA yourself, then scan again. The app retains your sign-in across launches.
2. Click **Name**, **Modified**, or **Size** to sort the entire collection. Results are displayed 100 at a time, but sorting and search cover every indexed file.
3. Search filenames and filter by type or folder. Folder filters include descendants. Active file names open the corresponding ChatGPT preview. Restore a trashed file before opening it.
4. Click a checkbox, then Shift-click another checkbox or filename to select a range in the current sorted/filtered list, including across pages. Select individual files or use the header checkbox to select **all matching files across pages**. The footer shows selected count and total size.
5. Choose **Review move to Trash**, inspect the complete selection, and confirm. Confirmed files are immediately hidden and unselected. While they process, select more files, review them, and choose **Add to queue**. Batches run in confirmation order, one file at a time with readback verification. **Stop all queued work** finishes the current file check and cancels the rest.
6. Open **Trash**, select files, and choose **Review restore**. Connect to Library first if opening Trash immediately after launch.
7. **Export CSV** writes the current inventory and a scan summary to `%LOCALAPPDATA%\GPTStorageBrowser\reports`. For file contents, open the ChatGPT preview and use its Download control; the app asks where to save the file.

## Features and boundaries

- Complete cursor pagination and recursive Library-folder traversal, including hidden files.
- Files and images in ChatGPT storage; connected-provider roots such as Google Drive are excluded.
- Compact file-browser layout with a narrow sidebar, inline file counts, dense rows, and batch details collapsed by default. Click the batch summary to inspect results; attention counts stay visible.
- Numeric size/date/name sorting, search, type/folder filters, cross-page multiselection and size totals.
- Small image/icon thumbnails beside filenames. Hover over a thumbnail for an enlarged image pop-out; moving away dismisses it without navigation. Clicking the filename still opens ChatGPT.
- The app-managed thumbnail and hover caches stay in memory; normal WebView2 browser data is kept in the private profile folder. Thumbnails fall back to original content when a thumbnail fails. Original-content previews fetch up to 32 MiB, fit the window, and cap decoded output at 2048 pixels per side; four enlarged previews are cached. Unsupported or oversized images show an unavailable message.
- Reviewed, queued bulk Trash and restore for owned ordinary Library files. Manage Pages, Sites, folders and shared items in ChatGPT's native interface.
- Per-file verification and action receipts. An uncertain or failed result stops the remaining files in that batch and cancels waiting batches. Unprocessed rows return to the list; the affected file remains blocked until refresh. No permanent-delete command.

Refresh if the collection changed elsewhere. Before changes, the app checks identity, ownership, name, parent, size, available timestamp and current Trash state. Failed scans remain visibly incomplete; bulk changes require an initial completed scan. Verified changes update the inventory, so another batch does not require rescanning. A failed or uncertain action blocks only that file until refresh; unaffected files remain available for a new reviewed batch. Queued work is not automatically resumed after closing the app or changing sign-in context. ChatGPT navigation and scanning are paused during processing so they cannot interrupt an active request. Quota usage can update separately from file visibility.

## Local data and connection

The app stores its profiles and generated reports under `%LOCALAPPDATA%\GPTStorageBrowser`, independently of the executable and checkout locations:

- `app-data/chatgpt-profile/`: authenticated ChatGPT browser profile.
- `app-data/interface/`: local UI WebView profile.
- `reports/`: exported metadata, diagnostic reports, and action receipts.

These files may contain credentials, filenames, and account identifiers. Keep them private. Chrome's profile is not imported or modified. If `LOCALAPPDATA` is missing or relative, startup fails instead of falling back to the project directory. Files saved through ChatGPT's native Download dialog go to the destination you choose.

Older installations that stored data beside the executable require a one-time migration of their `app-data` and `reports` directories into this private folder while the app is closed. Startup does not automatically search for or import an older profile.

The app learns the Library request context from the signed-in page. Session headers remain in memory and are sent only to ChatGPT. A changed sign-in context invalidates completed scans and pending reviews. Credentials are excluded from CSV, action receipts and diagnostics.

**Diagnostics** provides optional sanitized response schemas. ChatGPT's observed web endpoints are not a supported public personal-storage API, so service changes may require an app update. Native ChatGPT actions remain available in the embedded browser.

Explicit limits: 10,000 listing pages per scan, 1,000 Trash pages, 5,000 files per reviewed batch, up to 20 waiting batches, and up to 10,000 reserved files across active and waiting work. Large/invalid responses and repeated cursors stop the operation rather than report a complete inventory. File-change verification may search up to 1,000 pages of 50 Trash items when normal metadata lookup is unavailable.

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
| [src/main.rs](src/main.rs) | Native windows, separate WebViews, IPC validation, scan/job state, and action receipts |
| [src/inventory.rs](src/inventory.rs) | Normalized metadata, folder traversal, pagination, and completeness |
| [src/service.rs](src/service.rs) | Authenticated browser requests and asynchronous JSON results |
| [src/capture.rs](src/capture.rs) / [src/schema.rs](src/schema.rs) | Session observation, bounded schema diagnostics, and URL boundaries |
| [src/changes.rs](src/changes.rs) / [src/preflight.js](src/preflight.js) | Reviewed soft-delete/restore, live metadata checks, and readback verification |
| [src/ui.html](src/ui.html) / [src/inventory.js](src/inventory.js) | Trusted local shell, sorting/filtering, range selection, and review dialogs |
| [src/preview.rs](src/preview.rs) / [src/thumbnails.js](src/thumbnails.js) | Image loading/rasterization, lazy thumbnails, hover state, and caches |
| [src/download.rs](src/download.rs) / [src/export.rs](src/export.rs) | Native save dialog and safe CSV cells |
| [src/icon.rs](src/icon.rs) / [build.rs](build.rs) | Shared icon rasterizer and Windows executable resource |
| [tests/preflight.cjs](tests/preflight.cjs) | Production-script regression tests using mocked browser/service objects |

### State and safety rules

- Only the local shell receives IPC. Authenticated content runs in a separate WebView and profile.
- Listing generations reject stale callbacks. A review nonce binds confirmation to the native-approved selection.
- Only one mutation is active across all batches. Confirmed files are reserved until processed, preventing duplicate queueing. The active operation nonce is independent from newer review-dialog nonces. Persist pending work before issuing it; verify remote state afterward. Retry readback, never an ambiguous mutation.
- An uncertain file is quarantined until refresh, while unrelated files remain usable. A changed session clears account-bound snapshots and reviews.
- UI pagination changes rendering only. Sorting, search, select-all, and Shift-click operate on the complete filtered snapshot.
- Preview epochs discard stale images, but native outstanding-request counts remain until callbacks finish. Cache resets do not cancel network operations.
- Keep real account values out of comments, fixtures, source documentation, and commits. Runtime receipts are private artifacts.

Module/function comments explain these rules near their implementations. When changing embedded browser scripts, run both Rust and Node checks: the Node harness executes extracted production script text rather than a separately maintained copy.
