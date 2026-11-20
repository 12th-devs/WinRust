//! A Control Center made from separate owned compositor surfaces. There is no
//! large painted panel behind the tiles: the desktop is visible in every gap.
use crate::{
    SNAPSHOT, STATUS, Work, model::Action, platform::wide, request, sc, state_host, surface,
};
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::*,
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::*,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "WinRust.MenuBar.ControlTile";
const KEY: u32 = WM_APP + 20;
const HOVER: u32 = WM_APP + 21;
thread_local! { static TILES: RefCell<Vec<HWND>> = const { RefCell::new(vec![]) }; }

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Wifi,
    Bluetooth,
    Cast,
    Media,
    Focus,
    Display,
    Project,
    Brightness,
    Sound,
    Accessibility,
    Calculator,
    Timer,
    Capture,
    Settings,
}
struct Button {
    hwnd: HWND,
    text: String,
    icon: bool,
    action: Action,
}
struct Tile {
    kind: Kind,
    dpi: u32,
    fonts: [HFONT; 3],
    buttons: Vec<Button>,
    slider: HWND,
    monitor: isize,
    hover: Option<usize>,
}
unsafe fn tile(hwnd: HWND) -> &'static mut Tile {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Tile)
}
pub fn is_open() -> bool {
    TILES.with_borrow(|tiles| !tiles.is_empty())
}
pub fn contains(hwnd: HWND) -> bool {
    TILES.with_borrow(|tiles| tiles.contains(&hwnd))
}
pub unsafe fn close() {
    let tiles = TILES.with_borrow_mut(std::mem::take);
    if !tiles.is_empty() {
        request(Work::Control(0));
    }
    for hwnd in tiles.into_iter().rev() {
        DestroyWindow(hwnd);
    }
}

pub unsafe fn open(owner: HWND, anchor: RECT) {
    if is_open() {
        close();
        return;
    }
    let name = wide(CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: GetModuleHandleW(null()),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    RegisterClassW(&class);
    let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    let work = info.rcWork;
    let dpi = GetDpiForWindow(owner)
        .max(96)
        .min(((work.bottom - work.top - 16) * 96 / 580).max(64) as u32)
        .min(((work.right - work.left - 24) * 96 / 412).max(64) as u32);
    let left = (anchor.right - sc(412, dpi)).clamp(
        work.left + sc(8, dpi),
        (work.right - sc(412, dpi) - sc(8, dpi)).max(work.left + sc(8, dpi)),
    );
    let top = (anchor.bottom + sc(12, dpi))
        .min(work.bottom - sc(580, dpi) - sc(8, dpi))
        .max(work.top);
    let specs = [
        (Kind::Wifi, 0, 0, 200, 80),
        (Kind::Bluetooth, 0, 92, 94, 80),
        (Kind::Cast, 106, 92, 94, 80),
        (Kind::Media, 212, 0, 200, 172),
        (Kind::Focus, 0, 184, 200, 72),
        (Kind::Display, 212, 184, 94, 72),
        (Kind::Project, 318, 184, 94, 72),
        (Kind::Brightness, 0, 268, 412, 86),
        (Kind::Sound, 0, 366, 412, 86),
        (Kind::Accessibility, 0, 464, 94, 72),
        (Kind::Calculator, 106, 464, 94, 72),
        (Kind::Timer, 212, 464, 94, 72),
        (Kind::Capture, 318, 464, 94, 72),
        (Kind::Settings, 124, 548, 164, 32),
    ];
    for (kind, x, y, width, height) in specs {
        let state = Box::new(Tile {
            kind,
            dpi,
            fonts: [
                surface::menu_font(dpi),
                surface::font(dpi, false, false),
                surface::control_icon_font(dpi),
            ],
            buttons: vec![],
            slider: null_mut(),
            monitor: monitor as isize,
            hover: None,
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            name.as_ptr(),
            wide(title(kind)).as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            left + sc(x, dpi),
            top + sc(y, dpi),
            sc(width, dpi),
            sc(height, dpi),
            owner,
            null_mut(),
            GetModuleHandleW(null()),
            Box::into_raw(state).cast(),
        );
        if hwnd.is_null() {
            continue;
        }
        TILES.with_borrow_mut(|tiles| tiles.push(hwnd));
        surface::apply(hwnd, true);
        controls(hwnd);
    }
    TILES.with_borrow(|tiles| {
        for &hwnd in tiles {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    });
    let first = TILES.with_borrow(|tiles| tiles.first().copied());
    if let Some(first) = first {
        SetForegroundWindow(first);
        if let Some(button) = tile(first).buttons.first() {
            SetFocus(button.hwnd);
        }
    }
    refresh();
    request(Work::Control(monitor as isize));
}

fn title(kind: Kind) -> &'static str {
    match kind {
        Kind::Wifi => "Wi-Fi",
        Kind::Bluetooth => "Bluetooth",
        Kind::Cast => "Mirroring",
        Kind::Media => "Now Playing",
        Kind::Focus => "Focus",
        Kind::Display => "Displays",
        Kind::Project => "Project",
        Kind::Brightness => "Display brightness",
        Kind::Sound => "Sound",
        Kind::Accessibility => "Accessibility",
        Kind::Calculator => "Calculator",
        Kind::Timer => "Timer",
        Kind::Capture => "Capture",
        Kind::Settings => "System Settings",
    }
}

unsafe fn add_button(
    hwnd: HWND,
    text: &str,
    icon: bool,
    action: Action,
    bounds: (i32, i32, i32, i32),
) {
    let state = tile(hwnd);
    let id = state.buttons.len();
    let (x, y, w, h) = bounds;
    let accessible = match &action {
        Action::Radio("wifi") => "Toggle Wi-Fi",
        Action::Radio(_) => "Toggle Bluetooth",
        Action::Media(0) => "Previous track",
        Action::Media(1) => "Play or pause",
        Action::Media(_) => "Next track",
        Action::Mute => "Mute or unmute",
        _ => {
            if icon {
                title(state.kind)
            } else {
                text
            }
        }
    };
    let button = CreateWindowExW(
        0,
        wide("BUTTON").as_ptr(),
        wide(accessible).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
        sc(x, state.dpi),
        sc(y, state.dpi),
        sc(w, state.dpi),
        sc(h, state.dpi),
        hwnd,
        (100 + id) as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SetWindowSubclass(button, Some(child_proc), 1, 0);
    state.buttons.push(Button {
        hwnd: button,
        text: text.into(),
        icon,
        action,
    });
}

unsafe fn slider(hwnd: HWND, x: i32, width: i32, name: &str) {
    let state = tile(hwnd);
    let slider = CreateWindowExW(
        0,
        wide("msctls_trackbar32").as_ptr(),
        wide(name).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | TBS_NOTICKS,
        sc(x, state.dpi),
        sc(43, state.dpi),
        sc(width, state.dpi),
        sc(30, state.dpi),
        hwnd,
        50 as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(slider, TBM_SETRANGEMAX, 0, 100);
    SendMessageW(slider, TBM_SETPAGESIZE, 0, 5);
    SetWindowSubclass(slider, Some(child_proc), 1, 1);
    state.slider = slider;
}

unsafe fn controls(hwnd: HWND) {
    match tile(hwnd).kind {
        Kind::Wifi => {
            add_button(
                hwnd,
                "\u{E701}",
                true,
                Action::Radio("wifi"),
                (10, 18, 40, 42),
            );
            add_button(
                hwnd,
                "\u{E76C}",
                true,
                Action::Uri("ms-settings:network-wifi"),
                (170, 22, 24, 36),
            );
        }
        Kind::Bluetooth => add_button(
            hwnd,
            "\u{E702}",
            true,
            Action::Radio("bluetooth"),
            (20, 5, 54, 44),
        ),
        Kind::Cast => add_button(
            hwnd,
            "\u{EC15}",
            true,
            Action::Keys(vec![VK_LWIN, b'K' as u16]),
            (20, 5, 54, 44),
        ),
        Kind::Media => {
            add_button(hwnd, "\u{E892}", true, Action::Media(0), (12, 119, 52, 40));
            add_button(hwnd, "\u{E768}", true, Action::Media(1), (74, 119, 52, 40));
            add_button(hwnd, "\u{E893}", true, Action::Media(2), (136, 119, 52, 40));
        }
        Kind::Focus => add_button(
            hwnd,
            "\u{E708}",
            true,
            Action::Uri("ms-settings:quiethours"),
            (10, 15, 42, 42),
        ),
        Kind::Display => add_button(
            hwnd,
            "\u{E7F4}",
            true,
            Action::Uri("ms-settings:display"),
            (20, 3, 54, 42),
        ),
        Kind::Project => add_button(
            hwnd,
            "\u{EBC6}",
            true,
            Action::Keys(vec![VK_LWIN, b'P' as u16]),
            (20, 3, 54, 42),
        ),
        Kind::Brightness => {
            add_button(
                hwnd,
                "\u{E713}",
                true,
                Action::Uri("ms-settings:display"),
                (366, 5, 32, 32),
            );
            slider(hwnd, 16, 380, "Display brightness");
        }
        Kind::Sound => {
            add_button(hwnd, "\u{E767}", true, Action::Mute, (8, 40, 34, 34));
            add_button(
                hwnd,
                "\u{E713}",
                true,
                Action::Uri("ms-settings:sound"),
                (366, 5, 32, 32),
            );
            slider(hwnd, 48, 348, "Output volume");
        }
        kind => {
            let (glyph, action) = match kind {
                Kind::Accessibility => ("\u{E776}", Action::Uri("ms-settings:easeofaccess")),
                Kind::Calculator => ("\u{E8EF}", Action::Uri("calculator:")),
                Kind::Timer => ("\u{E916}", Action::Uri("ms-clock:")),
                Kind::Capture => (
                    "\u{E722}",
                    Action::Keys(vec![VK_LWIN, VK_SHIFT, b'S' as u16]),
                ),
                _ => ("System Settings", Action::Uri("ms-settings:")),
            };
            if kind == Kind::Settings {
                add_button(hwnd, glyph, false, action, (2, 1, 160, 30));
            } else {
                add_button(hwnd, glyph, true, action, (20, 3, 54, 42));
            }
        }
    }
}

unsafe fn label(hwnd: HWND, dc: HDC, text: &str, bounds: (i32, i32, i32, i32), small: bool) {
    let state = tile(hwnd);
    let (x, y, w, h) = bounds;
    let draw = DRAWITEMSTRUCT {
        CtlType: ODT_STATIC,
        hwndItem: hwnd,
        hDC: dc,
        rcItem: RECT {
            left: sc(x, state.dpi),
            top: sc(y, state.dpi),
            right: sc(x + w, state.dpi),
            bottom: sc(y + h, state.dpi),
        },
        ..zeroed()
    };
    surface::draw_button(&draw, text, state.fonts[small as usize], false, None);
}

unsafe fn paint(hwnd: HWND) {
    surface::paint_background(hwnd);
    let state = tile(hwnd);
    let status = STATUS.with_borrow(Clone::clone);
    let dc = GetDC(hwnd);
    match state.kind {
        Kind::Wifi => {
            label(hwnd, dc, "Wi-Fi", (60, 12, 108, 24), false);
            label(
                hwnd,
                dc,
                status.wifi.strip_prefix("Wi-Fi: ").unwrap_or(&status.wifi),
                (60, 38, 108, 22),
                true,
            );
        }
        Kind::Media => {
            label(hwnd, dc, "Now Playing", (16, 12, 168, 24), false);
            label(
                hwnd,
                dc,
                if status.media.available {
                    &status.media.title
                } else {
                    "No active media"
                },
                (16, 48, 168, 26),
                false,
            );
            label(hwnd, dc, &status.media.artist, (16, 78, 168, 22), true);
        }
        Kind::Focus => {
            label(hwnd, dc, "Focus", (62, 12, 120, 24), false);
            label(hwnd, dc, "Open settings", (62, 36, 120, 20), true);
        }
        Kind::Bluetooth => {
            label(hwnd, dc, "Bluetooth", (8, 43, 80, 18), true);
            label(
                hwnd,
                dc,
                status
                    .bluetooth
                    .strip_prefix("Bluetooth ")
                    .unwrap_or("Checking…"),
                (8, 61, 80, 17),
                true,
            );
        }
        Kind::Brightness => {
            let text = status
                .brightness
                .filter(|(m, _)| *m == state.monitor)
                .map_or("Display · settings only".into(), |(_, v)| {
                    format!("Display · {v}%")
                });
            label(hwnd, dc, &text, (16, 8, 340, 27), false);
        }
        Kind::Sound => {
            let text = status
                .volume
                .map_or("Sound unavailable".into(), |(v, muted)| {
                    format!("Sound · {v}%{}", if muted { " · muted" } else { "" })
                });
            label(hwnd, dc, &text, (16, 8, 340, 27), false);
        }
        Kind::Settings => {}
        _ => label(hwnd, dc, title(state.kind), (8, 48, 80, 22), true),
    }
    ReleaseDC(hwnd, dc);
}

pub unsafe fn refresh() {
    let status = STATUS.with_borrow(Clone::clone);
    TILES.with_borrow(|tiles| {
        for &hwnd in tiles {
            let state = tile(hwnd);
            let accessible = match state.kind {
                Kind::Wifi => format!("Wi-Fi controls, {}", status.wifi),
                Kind::Bluetooth => status.bluetooth.clone(),
                Kind::Media => {
                    if status.media.available {
                        format!(
                            "Now Playing, {}, {}",
                            status.media.title, status.media.artist
                        )
                    } else {
                        "Now Playing, no active media".into()
                    }
                }
                Kind::Sound => status
                    .volume
                    .map_or("Sound unavailable".into(), |(v, muted)| {
                        format!("Sound, {v} percent{}", if muted { ", muted" } else { "" })
                    }),
                _ => title(state.kind).into(),
            };
            SetWindowTextW(hwnd, wide(&accessible).as_ptr());
            if state.kind == Kind::Media {
                for (index, button) in state.buttons.iter_mut().enumerate() {
                    EnableWindow(button.hwnd, status.media.available as i32);
                    if index == 1 {
                        button.text = if status.media.playing {
                            "\u{E769}"
                        } else {
                            "\u{E768}"
                        }
                        .into();
                    }
                    InvalidateRect(button.hwnd, null(), 0);
                }
            }
            if state.kind == Kind::Sound
                && let Some(button) = state.buttons.first_mut()
            {
                button.text = if status.volume.is_some_and(|(_, muted)| muted) {
                    "\u{E74F}"
                } else {
                    "\u{E767}"
                }
                .into();
                InvalidateRect(button.hwnd, null(), 0);
            }
            if !state.slider.is_null() {
                let value = if state.kind == Kind::Sound {
                    status.volume.map(|(v, _)| v)
                } else {
                    status
                        .brightness
                        .filter(|(m, _)| *m == state.monitor)
                        .map(|(_, v)| v)
                };
                EnableWindow(state.slider, value.is_some() as i32);
                if let Some(value) = value
                    && GetCapture() != state.slider
                {
                    SendMessageW(state.slider, TBM_SETPOS, 1, value as isize);
                }
            }
            InvalidateRect(hwnd, null(), 0);
        }
    });
}

unsafe fn perform(action: Action) {
    if !matches!(
        action,
        Action::Radio(_)
            | Action::Media(_)
            | Action::Mute
            | Action::VolumeSet(_)
            | Action::BrightnessSet(_, _)
    ) {
        close();
    }
    request(Work::Action(action, SNAPSHOT.with_borrow(|s| s.target)));
}

unsafe fn next_focus(backwards: bool) {
    let handles = TILES.with_borrow(|tiles| {
        let mut handles = vec![];
        for &hwnd in tiles {
            let state = tile(hwnd);
            handles.extend(state.buttons.iter().map(|button| button.hwnd));
            if !state.slider.is_null() {
                handles.push(state.slider);
            }
        }
        handles.retain(|&h| IsWindowEnabled(h) != 0);
        handles
    });
    if handles.is_empty() {
        return;
    }
    let current = handles.iter().position(|&h| h == GetFocus()).unwrap_or(0);
    let next = (current as isize + if backwards { -1 } else { 1 })
        .rem_euclid(handles.len() as isize) as usize;
    SetForegroundWindow(GetParent(handles[next]));
    SetFocus(handles[next]);
}

unsafe extern "system" fn child_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    slider: usize,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND if slider != 0 && !surface::high_contrast() => return 1,
        WM_PAINT if slider != 0 && !surface::high_contrast() => {
            // Keep the native trackbar's input and accessibility, but paint its
            // transparent compositor surface ourselves before the themed class
            // can fill the child HWND with an opaque background.
            surface::paint_slider(hwnd);
            return 0;
        }
        WM_KEYDOWN if wp as u16 == VK_ESCAPE => {
            PostMessageW(state_host(), WM_CANCELMODE, 0, 0);
            return 0;
        }
        WM_KEYDOWN if wp as u16 == VK_TAB => {
            next_focus(GetKeyState(VK_SHIFT as i32) < 0);
            return 0;
        }
        WM_KEYDOWN if slider == 0 && matches!(wp as u16, VK_RETURN | VK_SPACE) => {
            SendMessageW(GetParent(hwnd), KEY, GetDlgCtrlID(hwnd) as usize, 0);
            return 0;
        }
        WM_GETDLGCODE if slider == 0 => return DLGC_WANTALLKEYS as isize,
        WM_MOUSEMOVE if slider == 0 => {
            SendMessageW(GetParent(hwnd), HOVER, GetDlgCtrlID(hwnd) as usize, 0);
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
        }
        WM_MOUSELEAVE => {
            SendMessageW(GetParent(hwnd), HOVER, usize::MAX, 0);
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            InvalidateRect(hwnd, null(), 0);
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(child_proc), 1);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return 1;
    }
    if GetWindowLongPtrW(hwnd, GWLP_USERDATA) == 0 {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_NCCALCSIZE => 0,
        WM_NCACTIVATE => DefWindowProcW(hwnd, msg, 1, lp),
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_DRAWITEM => {
            let draw = &*(lp as *const DRAWITEMSTRUCT);
            let state = tile(hwnd);
            if let Some(button) = state.buttons.get(draw.CtlID.saturating_sub(100) as usize) {
                surface::draw_button(
                    draw,
                    &button.text,
                    state.fonts[if button.icon { 2 } else { 1 }],
                    state.hover == Some(draw.CtlID.saturating_sub(100) as usize),
                    None,
                );
            }
            1
        }
        WM_COMMAND | KEY => {
            if msg == KEY || wp >> 16 == BN_CLICKED as usize {
                let index = (wp & 0xffff).saturating_sub(100);
                if let Some(button) = tile(hwnd).buttons.get(index) {
                    perform(button.action.clone());
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let state = tile(hwnd);
            let action = match state.kind {
                Kind::Wifi => Some(Action::Uri("ms-settings:network-wifi")),
                Kind::Sound => Some(Action::Uri("ms-settings:sound")),
                Kind::Brightness => Some(Action::Uri("ms-settings:display")),
                Kind::Media => None,
                _ => state.buttons.first().map(|button| button.action.clone()),
            };
            if let Some(action) = action {
                perform(action);
            }
            0
        }
        WM_HSCROLL => {
            let state = tile(hwnd);
            if lp as HWND == state.slider {
                let level = SendMessageW(state.slider, WM_USER, 0, 0) as i32;
                perform(if state.kind == Kind::Sound {
                    Action::VolumeSet(level)
                } else {
                    Action::BrightnessSet(state.monitor, level)
                });
            }
            0
        }
        HOVER => {
            let state = tile(hwnd);
            let next = wp.checked_sub(100).filter(|&i| i < state.buttons.len());
            if state.hover != next {
                let old = state.hover;
                state.hover = next;
                for i in [old, next].into_iter().flatten() {
                    InvalidateRect(state.buttons[i].hwnd, null(), 0);
                }
            }
            0
        }
        WM_ACTIVATE if wp & 0xffff == WA_INACTIVE as usize => {
            let other = lp as HWND;
            if !contains(other) && !crate::BARS.with_borrow(|bars| bars.contains(&other)) {
                PostMessageW(state_host(), WM_CANCELMODE, 0, 0);
            }
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            surface::apply(hwnd, true);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_CLOSE => {
            close();
            0
        }
        WM_NCDESTROY => {
            let state = Box::from_raw(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Tile);
            for font in state.fonts {
                DeleteObject(font);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
