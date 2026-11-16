//! Native save-path selection for downloads initiated in ChatGPT.
//!
//! Cancelling the dialog cancels the download. This module chooses a destination;
//! WebView2 performs the transfer and reports completion separately.

use std::path::PathBuf;
use windows::{
    Win32::{Foundation::HWND, UI::Controls::Dialogs::*},
    core::{PWSTR, w},
};

/// Ask for a destination with overwrite confirmation and an existing parent folder.
/// Returns false on cancel/failure and leaves the proposed path unchanged.
pub fn choose_path(owner: isize, path: &mut PathBuf) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
    let mut buffer = vec![0u16; 32768];
    for (to, from) in buffer.iter_mut().take(32766).zip(name.encode_utf16()) {
        *to = from;
    }
    let mut info = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut _),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrFilter: w!("All files\0*.*\0\0"),
        lpstrTitle: w!("Save file from ChatGPT"),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: info points into buffer, which remains allocated for the entire modal call;
    // nMaxFile is its capacity in UTF-16 code units, including the terminating null.
    if unsafe { GetSaveFileNameW(&mut info) }.as_bool() {
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        *path = PathBuf::from(String::from_utf16_lossy(&buffer[..end]));
        true
    } else {
        false
    }
}
