//! Keeps pinned notes above other always-on-top windows.
//!
//! Windows only orders topmost windows among themselves by who was activated
//! last, and Chrome can be topmost too, so the flag alone doesn't hold a note
//! in front. A note has to climb back when one of those windows covers it.

use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, PostMessageW, SetWindowPos,
    GWL_EXSTYLE, GW_HWNDPREV, GW_OWNER, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    WM_NCACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

const CHECK_INTERVAL: Duration = Duration::from_millis(500);

pub fn keep_pinned_on_top(app: AppHandle) {
    loop {
        thread::sleep(CHECK_INTERVAL);
        let windows = app.webview_windows();
        let notes: Vec<HWND> = windows
            .values()
            .filter_map(|window| window.hwnd().ok())
            .map(|hwnd| hwnd.0 as HWND)
            .collect();
        for window in windows.values().filter(|window| window.label() != crate::SETTINGS) {
            if let Ok(hwnd) = window.hwnd() {
                keep_backdrop(hwnd.0 as HWND);
            }
        }
        for &note in &notes {
            // SAFETY: the calls only read or reorder window handles, and Windows
            // rejects a handle that has since been destroyed.
            unsafe {
                if is_topmost(note) && is_covered(note, &notes) {
                    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                    SetWindowPos(note, HWND_TOPMOST, 0, 0, 0, 0, flags);
                }
            }
        }
    }
}

/// Windows turns a backdrop solid on inactive windows, which is most of a
/// note's life. Telling the window it still looks active keeps the glass, and
/// leaves keyboard focus where it is.
pub fn keep_backdrop(hwnd: HWND) {
    // SAFETY: posting to a destroyed handle fails harmlessly.
    unsafe {
        PostMessageW(hwnd, WM_NCACTIVATE, 1, 0);
    }
}

unsafe fn ex_style(hwnd: HWND) -> u32 {
    GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32
}

unsafe fn is_topmost(hwnd: HWND) -> bool {
    ex_style(hwnd) & WS_EX_TOPMOST != 0
}

/// Menus, tooltips and dialogs are owned or tool windows; those stay in front.
unsafe fn is_rival(hwnd: HWND) -> bool {
    IsWindowVisible(hwnd) != 0
        && is_topmost(hwnd)
        && ex_style(hwnd) & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) == 0
        && GetWindow(hwnd, GW_OWNER).is_null()
}

unsafe fn rect(hwnd: HWND) -> RECT {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    GetWindowRect(hwnd, &mut rect);
    rect
}

fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

unsafe fn is_covered(note: HWND, notes: &[HWND]) -> bool {
    let area = rect(note);
    let mut above = GetWindow(note, GW_HWNDPREV);
    while !above.is_null() {
        if !notes.contains(&above) && is_rival(above) && overlaps(&area, &rect(above)) {
            return true;
        }
        above = GetWindow(above, GW_HWNDPREV);
    }
    false
}
