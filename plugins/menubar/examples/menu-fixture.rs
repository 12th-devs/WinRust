//! A controlled application for testing the real cross-process HMENU adapter.
#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            SetWindowTextW(
                hwnd,
                wide(&format!("Menu fixture — command {} received", wp & 0xffff)).as_ptr(),
            );
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
fn main() {
    unsafe {
        let name = wide("WinRust.MenuFixture");
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
            ..zeroed()
        };
        RegisterClassW(&class);
        let root = CreateMenu();
        let file = CreatePopupMenu();
        let submenu = CreatePopupMenu();
        AppendMenuW(file, MF_STRING, 101, wide("&Test command\tCtrl+T").as_ptr());
        AppendMenuW(
            file,
            MF_STRING | MF_GRAYED,
            102,
            wide("Disabled command").as_ptr(),
        );
        AppendMenuW(
            file,
            MF_STRING | MF_CHECKED,
            103,
            wide("Checked command").as_ptr(),
        );
        AppendMenuW(file, MF_SEPARATOR, 0, null());
        AppendMenuW(submenu, MF_STRING, 104, wide("Nested command").as_ptr());
        AppendMenuW(file, MF_POPUP, submenu as usize, wide("Submenu").as_ptr());
        AppendMenuW(root, MF_POPUP, file as usize, wide("&File").as_ptr());
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("WinRust Menu Fixture").as_ptr(),
            WS_OVERLAPPEDWINDOW,
            180,
            160,
            650,
            320,
            null_mut(),
            root,
            GetModuleHandleW(null()),
            null(),
        );
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        let mut msg = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
