#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
mod config;
mod control;
mod model;
mod platform;
mod surface;

use config::Config;
use model::{Action, Item};
use platform::{Snapshot, wide};
use std::{
    cell::{Cell, RefCell},
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        Mutex,
        atomic::{AtomicIsize, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{
        Accessibility::*, Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*,
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "WinRust.MenuBar";
const POPUP: &str = "WinRust.MenuBar.Popup";
const REFRESH: u32 = WM_APP + 1;
const UPDATED: u32 = WM_APP + 2;
const APPBAR: u32 = WM_APP + 3;
const KEY: u32 = WM_APP + 4;
const HOVER: u32 = WM_APP + 5;
const REBUILD: u32 = WM_APP + 6;
const TBM_GETPOS: u32 = WM_USER;
static HOST: AtomicIsize = AtomicIsize::new(0);
static LATEST: Mutex<Option<Update>> = Mutex::new(None);
thread_local! {
    static BARS: RefCell<Vec<HWND>> = const { RefCell::new(vec![]) };
    static PANELS: RefCell<Vec<HWND>> = const { RefCell::new(vec![]) };
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    static SNAPSHOT: RefCell<Snapshot> = RefCell::new(Snapshot { target: 0, name: "Desktop".into(), menus: vec![], fullscreen: false });
    static STATUS: RefCell<Status> = RefCell::new(Status::default());
    static SENDER: RefCell<Option<std::sync::mpsc::Sender<Work>>> = const { RefCell::new(None) };
    static OPEN: Cell<Option<(HWND, usize)>> = const { Cell::new(None) };
    static POSITIONING: Cell<bool> = const { Cell::new(false) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Status {
    wifi: String,
    bluetooth: String,
    battery: Option<(u8, bool)>,
    volume: Option<(i32, bool)>,
    media: platform::Media,
    brightness: Option<(isize, i32)>,
}
struct Update {
    snapshot: Snapshot,
    status: Status,
}
enum Work {
    Refresh(isize),
    Menu(isize, usize),
    Action(Action, isize),
    Status,
    Control(isize),
}
struct Trigger {
    label: String,
    accessible: String,
    icon: bool,
    bold: bool,
    items: Vec<Item>,
    action: Action,
    app_index: Option<usize>,
}
struct Bar {
    monitor: HMONITOR,
    dpi: u32,
    rect: RECT,
    buttons: Vec<HWND>,
    triggers: Vec<Trigger>,
    fonts: [HFONT; 3],
    registered: bool,
    hidden: bool,
    clock: String,
    hover: i32,
}
struct Panel {
    items: Vec<Item>,
    buttons: Vec<HWND>,
    dpi: u32,
    font: HFONT,
    selected: usize,
    depth: usize,
    owner: HWND,
    target: isize,
    hover: Option<usize>,
    slider: HWND,
    offset: i32,
    max_scroll: i32,
}

fn sc(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}
unsafe fn bar(hwnd: HWND) -> &'static mut Bar {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Bar)
}
unsafe fn panel(hwnd: HWND) -> &'static mut Panel {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Panel)
}
fn request(work: Work) {
    SENDER.with_borrow(|sender| {
        if let Some(sender) = sender {
            let _ = sender.send(work);
        }
    });
}

unsafe extern "system" fn foreground(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if hwnd.is_null() {
        return;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid != GetCurrentProcessId() {
        PostMessageW(
            HOST.load(Ordering::Relaxed) as HWND,
            REFRESH,
            hwnd as usize,
            0,
        );
    }
}

fn start_worker() {
    let (sender, receiver) = std::sync::mpsc::channel();
    SENDER.with_borrow_mut(|slot| *slot = Some(sender));
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let _subscriptions = platform::Subscriptions::new();
        let mut target = unsafe { GetForegroundWindow() as isize };
        let mut status = Status::default();
        let mut control_monitor = 0;
        let mut last_status = std::time::Instant::now() - Duration::from_secs(60);
        let mut pending = None;
        loop {
            let work = pending.take().map_or_else(
                || {
                    receiver.recv_timeout(Duration::from_secs(if control_monitor == 0 {
                        30
                    } else {
                        3
                    }))
                },
                Ok,
            );
            match work {
                Ok(Work::Refresh(next)) => {
                    if next != 0 {
                        target = next;
                    }
                }
                Ok(Work::Menu(next, index)) => {
                    target = next;
                    platform::initialize_menu(target, index);
                }
                Ok(Work::Action(mut action, next)) => {
                    // Slider positions supersede older pending positions; discrete actions
                    // retain FIFO order and are never dropped during a drag.
                    if matches!(action, Action::VolumeSet(_) | Action::BrightnessSet(_, _)) {
                        while let Ok(work) = receiver.try_recv() {
                            match work {
                                Work::Action(new, _)
                                    if matches!(
                                        (&action, &new),
                                        (Action::VolumeSet(_), Action::VolumeSet(_))
                                    ) || matches!((&action, &new), (Action::BrightnessSet(a, _), Action::BrightnessSet(b, _)) if a == b) =>
                                {
                                    action = new
                                }
                                Work::Status => {}
                                other => {
                                    pending = Some(other);
                                    break;
                                }
                            }
                        }
                    }
                    platform::execute(&action, next);
                    match action {
                        Action::Volume(_) | Action::VolumeSet(_) | Action::Mute => {
                            status.volume = platform::volume(None, false)
                        }
                        Action::BrightnessSet(monitor, _) => {
                            status.brightness =
                                platform::brightness(monitor, None).map(|v| (monitor, v))
                        }
                        Action::Media(_) => status.media = platform::media(None),
                        Action::Radio(_) | Action::None => {
                            last_status = std::time::Instant::now() - Duration::from_secs(60)
                        }
                        _ => {}
                    }
                }
                Ok(Work::Status) => {
                    status.volume = platform::volume(None, false);
                    status.wifi = platform::wifi();
                }
                Ok(Work::Control(monitor)) => {
                    control_monitor = monitor;
                    last_status = std::time::Instant::now() - Duration::from_secs(60);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(_) => {
                    if control_monitor != 0 {
                        status.media = platform::media(None);
                    }
                }
            }
            if last_status.elapsed() >= Duration::from_secs(30) {
                status = Status {
                    wifi: platform::wifi(),
                    bluetooth: platform::bluetooth(),
                    battery: platform::battery(),
                    volume: platform::volume(None, false),
                    media: if control_monitor != 0 {
                        platform::media(None)
                    } else {
                        platform::Media::default()
                    },
                    brightness: if control_monitor != 0 {
                        platform::brightness(control_monitor, None).map(|v| (control_monitor, v))
                    } else {
                        None
                    },
                };
                last_status = std::time::Instant::now();
            }
            let snapshot = platform::snapshot(target);
            *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Update {
                snapshot,
                status: status.clone(),
            });
            unsafe {
                PostMessageW(HOST.load(Ordering::Relaxed) as HWND, UPDATED, 0, 0);
            }
        }
    });
}

pub fn notify_status() {
    unsafe {
        PostMessageW(state_host(), WM_APP + 7, 0, 0);
    }
}

fn system_items() -> Vec<Item> {
    vec![
        Item::new("About this PC", Action::Uri("ms-settings:about")),
        Item::new("System Settings…", Action::Uri("ms-settings:")),
        Item::new("Notifications", Action::Keys(vec![VK_LWIN, b'N' as u16])),
        Item::new("Microsoft Store", Action::Uri("ms-windows-store:")),
        Item::separator(),
        Item::new(
            "Task Manager",
            Action::Keys(vec![VK_CONTROL, VK_SHIFT, VK_ESCAPE]),
        ),
        Item::new("Lock screen", Action::Lock),
        Item::new("Power options…", Action::Uri("ms-settings:powersleep")),
        Item::separator(),
        preferences(),
        Item::new("Quit Menu Bar", Action::Exit),
    ]
}

fn preferences() -> Item {
    CONFIG.with_borrow(|config| {
        let mut children: Vec<Item> = [
            ("Show seconds", "seconds", config.seconds),
            ("Auto-hide", "auto_hide", config.auto_hide),
            ("Show Wi-Fi", "wifi", config.wifi),
            ("Show Bluetooth", "bluetooth", config.bluetooth),
            ("Show battery", "battery", config.battery),
        ]
        .into_iter()
        .map(|(label, key, checked)| Item {
            checked,
            ..Item::new(label, Action::Toggle(key))
        })
        .collect();
        children.insert(
            0,
            Item::group(
                "Background material",
                vec![
                    Item {
                        checked: !config.acrylic,
                        ..Item::new("Mica · wallpaper tint", Action::Toggle("mica"))
                    },
                    Item {
                        checked: config.acrylic,
                        ..Item::new("Acrylic · frosted blur", Action::Toggle("acrylic"))
                    },
                ],
            ),
        );
        Item::group("Menu Bar Settings", children)
    })
}

fn window_items() -> Vec<Item> {
    let valid = SNAPSHOT.with_borrow(|s| s.target != 0 && s.name != "Desktop");
    [
        ("Restore", SC_RESTORE),
        ("Minimize", SC_MINIMIZE),
        ("Maximize", SC_MAXIMIZE),
        ("Close window\tAlt+F4", SC_CLOSE),
    ]
    .into_iter()
    .map(|(label, command)| Item {
        enabled: valid,
        ..Item::new(label, Action::System(command))
    })
    .collect()
}

fn control_items() -> Vec<Item> {
    STATUS.with_borrow(|status| {
        let volume = status
            .volume
            .map(|(v, muted)| format!("Sound: {v}%{}", if muted { " · muted" } else { "" }))
            .unwrap_or_else(|| "Sound unavailable".into());
        vec![
            Item::new(
                if status.wifi.is_empty() {
                    "Wi-Fi"
                } else {
                    &status.wifi
                },
                Action::Uri("ms-settings:network-wifi"),
            ),
            Item::new(
                if status.bluetooth.is_empty() {
                    "Bluetooth"
                } else {
                    &status.bluetooth
                },
                Action::Uri("ms-settings:bluetooth"),
            ),
            Item::separator(),
            Item::group(
                volume,
                vec![
                    Item {
                        enabled: status.volume.is_some(),
                        ..Item::new(
                            "Output volume",
                            Action::VolumeSet(status.volume.map_or(0, |(v, _)| v)),
                        )
                    },
                    Item {
                        enabled: status.volume.is_some(),
                        ..Item::new("Volume up\t+5%", Action::Volume(5))
                    },
                    Item {
                        enabled: status.volume.is_some(),
                        ..Item::new("Volume down\t−5%", Action::Volume(-5))
                    },
                    Item {
                        enabled: status.volume.is_some(),
                        checked: status.volume.is_some_and(|(_, muted)| muted),
                        ..Item::new("Mute", Action::Mute)
                    },
                    Item::new("Sound devices…", Action::Uri("ms-settings:sound")),
                ],
            ),
            Item::new("Display & brightness…", Action::Uri("ms-settings:display")),
            Item::new("Focus…", Action::Uri("ms-settings:quiethours")),
            Item::new(
                "Screen mirroring…",
                Action::Keys(vec![VK_LWIN, b'K' as u16]),
            ),
            Item::separator(),
            Item::new(
                "Windows Quick Settings",
                Action::Keys(vec![VK_LWIN, b'A' as u16]),
            ),
            Item::new("Accessibility…", Action::Uri("ms-settings:easeofaccess")),
        ]
    })
}

/// Update live audio values without replacing focused native controls.
unsafe fn refresh_controls() {
    let volume = STATUS.with_borrow(|s| s.volume);
    PANELS.with_borrow(|panels| {
        for &hwnd in panels {
            let state = panel(hwnd);
            for (index, item) in state.items.iter_mut().enumerate() {
                if item.label.starts_with("Sound:")
                    && let Some((level, muted)) = volume
                {
                    item.label = format!("Sound: {level}%{}", if muted { " · muted" } else { "" });
                }
                if matches!(item.action, Action::Mute) {
                    item.checked = volume.is_some_and(|(_, muted)| muted);
                }
                if let Some(&button) = state.buttons.get(index)
                    && !button.is_null()
                {
                    let label = format!(
                        "{}{}{}",
                        if item.checked { "Checked, " } else { "" },
                        item.label.replace('\t', ", shortcut "),
                        if item.children.is_empty() {
                            ""
                        } else {
                            ", submenu"
                        }
                    );
                    SetWindowTextW(button, wide(&label).as_ptr());
                    InvalidateRect(button, null(), 0);
                }
            }
            if !state.slider.is_null()
                && GetFocus() != state.slider
                && let Some((level, _)) = volume
            {
                SendMessageW(state.slider, TBM_SETPOS, 1, level as isize);
            }
        }
    });
}

fn trigger(label: &str, accessible: &str, icon: bool, items: Vec<Item>, action: Action) -> Trigger {
    Trigger {
        label: label.into(),
        accessible: accessible.into(),
        icon,
        bold: false,
        items,
        action,
        app_index: None,
    }
}

unsafe fn layout(hwnd: HWND) {
    let state = bar(hwnd);
    for button in state.buttons.drain(..) {
        DestroyWindow(button);
    }
    let config = CONFIG.with_borrow(Clone::clone);
    let snapshot = SNAPSHOT.with_borrow(Clone::clone);
    let status = STATUS.with_borrow(Clone::clone);
    let mut left = vec![trigger(
        "windows",
        "System menu",
        true,
        system_items(),
        Action::None,
    )];
    let application = trigger(
        &snapshot.name,
        &format!("{} application menu", snapshot.name),
        false,
        window_items(),
        Action::None,
    );
    left.push(application);
    for (index, menu) in snapshot.menus.iter().enumerate() {
        let mut t = trigger(
            &menu.label,
            &menu.label,
            false,
            menu.children.clone(),
            menu.action.clone(),
        );
        t.app_index = Some(index);
        left.push(t);
    }
    let mut right = vec![];
    if config.wifi {
        right.push(trigger(
            "\u{E701}",
            if status.wifi.is_empty() {
                "Wi-Fi"
            } else {
                &status.wifi
            },
            true,
            vec![
                Item::new(&status.wifi, Action::Uri("ms-settings:network-wifi")),
                Item::new(
                    "Network settings…",
                    Action::Uri("ms-settings:network-status"),
                ),
            ],
            Action::None,
        ));
    }
    if config.bluetooth {
        right.push(trigger(
            "\u{E702}",
            if status.bluetooth.is_empty() {
                "Bluetooth"
            } else {
                &status.bluetooth
            },
            true,
            vec![Item::new(
                &status.bluetooth,
                Action::Uri("ms-settings:bluetooth"),
            )],
            Action::None,
        ));
    }
    if config.battery
        && let Some((level, ac)) = status.battery
    {
        right.push(trigger(
            &format!("{level}%{}", if ac { " +" } else { "" }),
            &format!(
                "Battery {level} percent{}",
                if ac { ", connected to power" } else { "" }
            ),
            false,
            vec![Item::new(
                "Battery settings…",
                Action::Uri("ms-settings:batterysaver"),
            )],
            Action::None,
        ));
    }
    right.push(trigger(
        if status.volume.is_some_and(|(_, muted)| muted) {
            "\u{E74F}"
        } else {
            "\u{E767}"
        },
        "Sound",
        true,
        control_items()
            .into_iter()
            .filter(|i| i.label.starts_with("Sound"))
            .collect(),
        Action::None,
    ));
    right.push(trigger(
        "\u{E721}",
        "Search",
        true,
        vec![],
        Action::Keys(vec![VK_LWIN, b'S' as u16]),
    ));
    right.push(trigger(
        "\u{E713}",
        "Control Center",
        true,
        vec![],
        Action::ControlCenter,
    ));
    let clock = platform::clock(config.seconds);
    right.push(trigger(
        &clock,
        &format!("{clock}, open Control Center"),
        false,
        vec![],
        Action::ControlCenter,
    ));
    state.clock = clock;
    let width = state.rect.right - state.rect.left;
    let height = sc(28, state.dpi);
    let item_width = |item: &Trigger| {
        if item.icon {
            sc(32, state.dpi)
        } else {
            surface::measure(
                hwnd,
                &item.label,
                state.fonts[if item.bold { 1 } else { 0 }],
            ) + sc(24, state.dpi)
        }
    };
    let mut right_widths: Vec<i32> = right.iter().map(item_width).collect();
    // Keep clock width stable across minute/AM-PM changes.
    if let Some(last) = right_widths.last_mut() {
        *last = (*last).max(sc(if config.seconds { 166 } else { 142 }, state.dpi));
    }
    // On narrow displays move optional status items into Control Center first.
    while right_widths.iter().sum::<i32>() + sc(220, state.dpi) > width && right.len() > 3 {
        right.remove(0);
        right_widths.remove(0);
    }
    let right_total: i32 = right_widths.iter().sum();
    let left_widths: Vec<i32> = left.iter().map(item_width).collect();
    let available = (width - right_total - sc(16, state.dpi)).max(0);
    let keep = model::visible_count(&left_widths, available, sc(36, state.dpi));
    if keep < left.len() {
        let overflow = left
            .drain(keep..)
            .map(|t| {
                if t.items.is_empty() {
                    Item::new(t.accessible, t.action)
                } else {
                    Item::group(t.accessible, t.items)
                }
            })
            .collect();
        left.push(trigger(
            "\u{E712}",
            "More application menus",
            true,
            overflow,
            Action::None,
        ));
    }
    let mut positions = vec![];
    let mut x = sc(6, state.dpi);
    for item in &left {
        let w = item_width(item).min((width - right_total - x).max(0));
        positions.push((x, w));
        x += w;
    }
    x = width - right_total - sc(4, state.dpi);
    for w in right_widths {
        positions.push((x, w));
        x += w;
    }
    state.triggers = left;
    state.triggers.extend(right);
    for (index, (x, w)) in positions.into_iter().enumerate() {
        let item = &state.triggers[index];
        let button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(&item.accessible).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            x,
            0,
            w,
            height,
            hwnd,
            (index + 100) as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        SetWindowSubclass(button, Some(button_proc), 1, 0);
        state.buttons.push(button);
    }
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn position(hwnd: HWND) {
    if POSITIONING.replace(true) {
        return;
    }
    let state = bar(hwnd);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(state.monitor, &mut info) != 0 {
        let dpi = GetDpiForWindow(hwnd).max(96);
        if state.dpi != dpi {
            for font in state.fonts {
                DeleteObject(font);
            }
            state.fonts = [
                surface::font(dpi, false, false),
                surface::font(dpi, true, false),
                surface::font(dpi, false, true),
            ];
            state.dpi = dpi;
        }
        let height = sc(28, state.dpi);
        let mut data: APPBARDATA = zeroed();
        data.cbSize = size_of::<APPBARDATA>() as u32;
        data.hWnd = hwnd;
        data.uEdge = ABE_TOP;
        data.rc = info.rcMonitor;
        data.rc.bottom = data.rc.top + height;
        if state.registered {
            SHAppBarMessage(ABM_QUERYPOS, &mut data);
            data.rc.bottom = data.rc.top + height;
            // Autohide releases the workspace except for its reveal edge.
            if CONFIG.with_borrow(|c| c.auto_hide) {
                data.rc.bottom = data.rc.top + 1;
            }
            SHAppBarMessage(ABM_SETPOS, &mut data);
        }
        state.rect = RECT {
            left: data.rc.left,
            top: data.rc.top,
            right: data.rc.right,
            bottom: data.rc.top + height,
        };
        let r = state.rect;
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            r.left,
            if state.hidden {
                r.top - height + 1
            } else {
                r.top
            },
            r.right - r.left,
            height,
            SWP_NOACTIVATE,
        );
    }
    POSITIONING.set(false);
}

unsafe fn close_panels(from: usize) {
    let windows = PANELS.with_borrow_mut(|panels| {
        if from < panels.len() {
            panels.drain(from..).collect::<Vec<_>>()
        } else {
            vec![]
        }
    });
    for hwnd in windows.into_iter().rev() {
        DestroyWindow(hwnd);
    }
    if from == 0 {
        control::close();
        OPEN.set(None);
        if DIRTY.replace(false) {
            PostMessageW(state_host(), WM_APP + 8, 0, 0);
        }
        BARS.with_borrow(|bars| {
            for &hwnd in bars {
                if !hwnd.is_null() {
                    InvalidateRect(hwnd, null(), 0);
                }
            }
        });
    }
}

unsafe fn open_panel(owner: HWND, items: Vec<Item>, anchor: RECT, depth: usize, target: isize) {
    close_panels(depth);
    if items.is_empty() {
        return;
    }
    let dpi = GetDpiForWindow(owner).max(96);
    let font = surface::menu_font(dpi);
    let width = items
        .iter()
        .map(|item| surface::measure(owner, &item.label.replace('\t', "    "), font) + sc(64, dpi))
        .max()
        .unwrap_or(sc(260, dpi))
        .clamp(sc(240, dpi), sc(620, dpi));
    let height = items
        .iter()
        .map(|item| {
            sc(
                if item.label.is_empty() {
                    9
                } else if matches!(item.action, Action::VolumeSet(_)) {
                    64
                } else {
                    32
                },
                dpi,
            )
        })
        .sum::<i32>()
        + sc(12, dpi);
    let mon = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(mon, &mut info);
    let work = info.rcWork;
    let mut x = if depth == 0 {
        anchor.left
    } else {
        anchor.right - sc(3, dpi)
    };
    let y = if depth == 0 {
        anchor.bottom + sc(4, dpi)
    } else {
        anchor.top - sc(6, dpi)
    };
    if x + width > work.right {
        x = if depth == 0 {
            work.right - width
        } else {
            anchor.left - width
        };
    }
    x = x.max(work.left);
    let full_height = height;
    let height = height.min(work.bottom - work.top);
    let y = y.min(work.bottom - height).max(work.top);
    let selected = items
        .iter()
        .position(|item| item.enabled && !item.label.is_empty())
        .unwrap_or(0);
    let state = Box::new(Panel {
        items,
        buttons: vec![],
        dpi,
        font,
        selected,
        depth,
        owner,
        target,
        hover: None,
        slider: null_mut(),
        offset: 0,
        max_scroll: full_height - height,
    });
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        wide(POPUP).as_ptr(),
        wide(if depth == 0 {
            "Menu Bar menu"
        } else {
            "Submenu"
        })
        .as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN | if full_height > height { WS_VSCROLL } else { 0 },
        x,
        y,
        width,
        height,
        owner,
        null_mut(),
        GetModuleHandleW(null()),
        Box::into_raw(state).cast(),
    );
    if hwnd.is_null() {
        DeleteObject(font);
        return;
    }
    PANELS.with_borrow_mut(|panels| panels.push(hwnd));
    surface::apply(hwnd, true);
    let state = panel(hwnd);
    let mut top = sc(6, dpi);
    for (index, item) in state.items.iter().enumerate() {
        if item.label.is_empty() {
            top += sc(9, dpi);
            state.buttons.push(null_mut());
            continue;
        }
        let label = format!(
            "{}{}{}",
            if item.checked { "Checked, " } else { "" },
            item.label.replace('\t', ", shortcut "),
            if item.children.is_empty() {
                ""
            } else {
                ", submenu"
            }
        );
        let button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(&label).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
            sc(5, dpi),
            top,
            width - sc(10, dpi),
            sc(32, dpi),
            hwnd,
            (index + 100) as HMENU,
            GetModuleHandleW(null()),
            null(),
        );
        if !item.enabled {
            EnableWindow(button, 0);
        }
        SetWindowSubclass(button, Some(button_proc), 1, 0);
        state.buttons.push(button);
        top += sc(32, dpi);
        if let Action::VolumeSet(level) = item.action {
            let slider = CreateWindowExW(
                0,
                wide("msctls_trackbar32").as_ptr(),
                wide("Output volume").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | TBS_HORZ | TBS_NOTICKS,
                sc(20, dpi),
                top,
                width - sc(40, dpi),
                sc(28, dpi),
                hwnd,
                50 as HMENU,
                GetModuleHandleW(null()),
                null(),
            );
            SendMessageW(slider, TBM_SETRANGEMIN, 0, 0);
            SendMessageW(slider, TBM_SETRANGEMAX, 0, 100);
            SendMessageW(slider, TBM_SETPOS, 1, level as isize);
            SendMessageW(slider, TBM_SETPAGESIZE, 0, 5);
            EnableWindow(slider, item.enabled as i32);
            state.slider = slider;
            top += sc(32, dpi);
            SetWindowSubclass(slider, Some(button_proc), 1, 1);
        }
    }
    if state.max_scroll > 0 {
        let scroll = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE,
            nMin: 0,
            nMax: full_height - 1,
            nPage: height as u32,
            ..zeroed()
        };
        SetScrollInfo(hwnd, SB_VERT, &scroll, 1);
    }
    ShowWindow(hwnd, SW_SHOWNORMAL);
    SetForegroundWindow(hwnd);
    if let Some(&button) = state.buttons.get(selected) {
        SetFocus(button);
    }
}

unsafe fn open_trigger(hwnd: HWND, index: usize, keyboard: bool) {
    let state = bar(hwnd);
    if index >= state.triggers.len() {
        return;
    }
    let item = &state.triggers[index];
    let target = SNAPSHOT.with_borrow(|s| s.target);
    let action = item.action.clone();
    let items = item.items.clone();
    let app_index = item.app_index;
    let mut rect = zeroed();
    GetWindowRect(state.buttons[index], &mut rect);
    if action == Action::ControlCenter {
        if control::is_open() {
            control::close();
        } else {
            close_panels(0);
            control::open(hwnd, rect);
        }
        return;
    }
    if items.is_empty() {
        close_panels(0);
        request(Work::Action(action, target));
        return;
    }
    close_panels(0);
    OPEN.set(Some((hwnd, index)));
    if let Some(index) = app_index {
        request(Work::Menu(target, index));
    }
    open_panel(hwnd, items, rect, 0, target);
    // open_panel resets OPEN while replacing its root.
    OPEN.set(Some((hwnd, index)));
    if !keyboard {
        InvalidateRect(state.buttons[index], null(), 0);
    }
}

unsafe fn invoke(hwnd: HWND, index: usize, keyboard: bool) {
    let state = panel(hwnd);
    let Some(item) = state.items.get(index).cloned() else {
        return;
    };
    if !item.enabled || item.label.is_empty() {
        return;
    }
    if !item.children.is_empty() {
        let mut rect = zeroed();
        GetWindowRect(state.buttons[index], &mut rect);
        open_panel(
            state.owner,
            item.children,
            rect,
            state.depth + 1,
            state.target,
        );
        return;
    }
    let owner = state.owner;
    let target = state.target;
    let depth = state.depth;
    match item.action {
        Action::Volume(_) | Action::Mute => {
            request(Work::Action(item.action, target));
            // Keep the control panel open; status updates refresh accessible values.
            if keyboard {
                SetFocus(state.buttons[index]);
            }
        }
        Action::VolumeSet(_) => {
            if !state.slider.is_null() {
                SetFocus(state.slider);
            }
        }
        Action::Toggle(key) => {
            CONFIG.with_borrow_mut(|config| config.toggle(key));
            close_panels(0);
            BARS.with_borrow(|bars| {
                for &bar in bars {
                    surface::apply(bar, false);
                    position(bar);
                    layout(bar);
                }
            });
        }
        Action::Exit => {
            close_panels(0);
            PostMessageW(HOST.load(Ordering::Relaxed) as HWND, WM_CLOSE, 0, 0);
        }
        action => {
            close_panels(0);
            SetForegroundWindow(target as HWND);
            request(Work::Action(action, target));
        }
    }
    let _ = (owner, depth);
}

unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let parent = GetParent(hwnd);
    match msg {
        WM_ERASEBKGND if data != 0 && !surface::high_contrast() => return 1,
        WM_PAINT if data != 0 && !surface::high_contrast() => {
            surface::paint_slider(hwnd);
            return 0;
        }
        WM_GETDLGCODE if data == 0 => return DLGC_WANTALLKEYS as isize,
        WM_KEYDOWN if data == 0 || matches!(wp as u16, VK_ESCAPE | VK_TAB) => {
            SendMessageW(parent, KEY, wp, GetDlgCtrlID(hwnd) as isize);
            return 0;
        }
        WM_MOUSEWHEEL => {
            SendMessageW(parent, WM_MOUSEWHEEL, wp, lp);
            return 0;
        }
        WM_MOUSEMOVE => {
            SendMessageW(parent, HOVER, GetDlgCtrlID(hwnd) as usize, 0);
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
        }
        WM_MOUSELEAVE => {
            SendMessageW(parent, HOVER, usize::MAX, 0);
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            InvalidateRect(hwnd, null(), 0);
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(button_proc), 1);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe fn scroll_panel(hwnd: HWND, delta: i32) {
    let state = panel(hwnd);
    let next = (state.offset + delta).clamp(0, state.max_scroll);
    let change = state.offset - next;
    state.offset = next;
    for &button in state.buttons.iter().chain(std::iter::once(&state.slider)) {
        if button.is_null() {
            continue;
        }
        let mut rect = zeroed();
        GetWindowRect(button, &mut rect);
        let mut point = POINT {
            x: rect.left,
            y: rect.top,
        };
        ScreenToClient(hwnd, &mut point);
        SetWindowPos(
            button,
            null_mut(),
            point.x,
            point.y + change,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let scroll = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_POS,
        nPos: next,
        ..zeroed()
    };
    SetScrollInfo(hwnd, SB_VERT, &scroll, 1);
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn reveal_row(hwnd: HWND, index: usize) {
    let state = panel(hwnd);
    let Some(&button) = state.buttons.get(index) else {
        return;
    };
    if button.is_null() {
        return;
    }
    let mut rect = zeroed();
    GetWindowRect(button, &mut rect);
    let mut point = POINT {
        x: rect.left,
        y: rect.top,
    };
    ScreenToClient(hwnd, &mut point);
    let mut client = zeroed();
    GetClientRect(hwnd, &mut client);
    if point.y < 5 {
        scroll_panel(hwnd, point.y - 5);
    } else if point.y + rect.bottom - rect.top > client.bottom - 5 {
        scroll_panel(hwnd, point.y + rect.bottom - rect.top - client.bottom + 5);
    }
}

unsafe fn navigate_panel(hwnd: HWND, key: u16, current: usize) {
    let state = panel(hwnd);
    let count = state.items.len();
    if count == 0 {
        return;
    }
    match key {
        VK_ESCAPE | VK_LEFT if state.depth > 0 => {
            let depth = state.depth;
            close_panels(depth);
            PANELS.with_borrow(|panels| {
                if let Some(&parent) = panels.last() {
                    let p = panel(parent);
                    SetForegroundWindow(parent);
                    if let Some(&b) = p.buttons.get(p.selected) {
                        SetFocus(b);
                    }
                }
            });
        }
        VK_ESCAPE => {
            let owner = state.owner;
            close_panels(0);
            SetForegroundWindow(owner);
            if let Some(&button) = bar(owner).buttons.first() {
                SetFocus(button);
            }
        }
        VK_UP | VK_DOWN | VK_HOME | VK_END | VK_TAB => {
            let direction: isize =
                if key == VK_UP || (key == VK_TAB && GetKeyState(VK_SHIFT as i32) < 0) {
                    -1
                } else {
                    1
                };
            let start = if key == VK_HOME {
                count - 1
            } else if key == VK_END {
                0
            } else {
                current
            };
            let direction = if key == VK_END { -1 } else { direction };
            for offset in 1..=count {
                let index = (start as isize + offset as isize * direction)
                    .rem_euclid(count as isize) as usize;
                if state.items[index].enabled && !state.items[index].label.is_empty() {
                    state.selected = index;
                    close_panels(state.depth + 1);
                    reveal_row(hwnd, index);
                    SetFocus(state.buttons[index]);
                    break;
                }
            }
        }
        VK_RETURN | VK_SPACE => invoke(hwnd, current, true),
        VK_RIGHT if !state.items[current].children.is_empty() => invoke(hwnd, current, true),
        VK_LEFT | VK_RIGHT => {
            if let Some((owner, index)) = OPEN.get() {
                let count = bar(owner).triggers.len();
                let next = (index as isize + if key == VK_LEFT { -1 } else { 1 })
                    .rem_euclid(count as isize) as usize;
                open_trigger(owner, next, true);
            }
        }
        0x30..=0x5A => {
            let c = char::from_u32(key as u32)
                .unwrap_or_default()
                .to_ascii_lowercase();
            for offset in 1..=count {
                let index = (current + offset) % count;
                if state.items[index].enabled
                    && state.items[index].label.to_lowercase().starts_with(c)
                {
                    state.selected = index;
                    reveal_row(hwnd, index);
                    SetFocus(state.buttons[index]);
                    break;
                }
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
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
        // A shell surface must retain its material while another app has focus.
        WM_NCACTIVATE => DefWindowProcW(hwnd, msg, 1, lp),
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            surface::paint_background(hwnd);
            // Separators are painted once between native accessible rows.
            let s = panel(hwnd);
            let dc = GetDC(hwnd);
            let mut y = sc(6, s.dpi) - s.offset;
            for item in &s.items {
                if item.label.is_empty() {
                    let brush = CreateSolidBrush(if surface::dark() {
                        0x00575757
                    } else {
                        0x00bdbdbd
                    });
                    FillRect(
                        dc,
                        &RECT {
                            left: sc(15, s.dpi),
                            top: y + sc(4, s.dpi),
                            right: {
                                let mut r = zeroed();
                                GetClientRect(hwnd, &mut r);
                                r.right - sc(15, s.dpi)
                            },
                            bottom: y + sc(4, s.dpi) + 1,
                        },
                        brush,
                    );
                    DeleteObject(brush);
                    y += sc(9, s.dpi);
                } else {
                    y += sc(
                        if matches!(item.action, Action::VolumeSet(_)) {
                            64
                        } else {
                            32
                        },
                        s.dpi,
                    );
                }
            }
            ReleaseDC(hwnd, dc);
            0
        }
        WM_DRAWITEM => {
            let draw = &*(lp as *const DRAWITEMSTRUCT);
            let state = panel(hwnd);
            let index = draw.CtlID.saturating_sub(100) as usize;
            if let Some(item) = state.items.get(index) {
                surface::draw_button(
                    draw,
                    &item.label,
                    state.font,
                    state
                        .hover
                        .map_or(draw.itemState & ODS_FOCUS != 0, |hover| hover == index),
                    Some(item),
                );
            }
            1
        }
        WM_COMMAND => {
            if (wp >> 16) == BN_CLICKED as usize {
                invoke(hwnd, (wp & 0xffff).saturating_sub(100), false);
            }
            0
        }
        WM_HSCROLL => {
            let state = panel(hwnd);
            if lp as HWND == state.slider && !state.slider.is_null() {
                let level = SendMessageW(state.slider, TBM_GETPOS, 0, 0) as i32;
                request(Work::Action(Action::VolumeSet(level), state.target));
            }
            0
        }
        WM_MOUSEWHEEL => {
            scroll_panel(
                hwnd,
                -((wp >> 16) as i16 as i32) / 120 * sc(84, panel(hwnd).dpi),
            );
            0
        }
        WM_VSCROLL => {
            let state = panel(hwnd);
            let delta = match (wp & 0xffff) as i32 {
                SB_LINEUP => -sc(32, state.dpi),
                SB_LINEDOWN => sc(32, state.dpi),
                SB_PAGEUP => -sc(224, state.dpi),
                SB_PAGEDOWN => sc(224, state.dpi),
                SB_THUMBTRACK | SB_THUMBPOSITION => (wp >> 16) as i32 - state.offset,
                _ => 0,
            };
            scroll_panel(hwnd, delta);
            0
        }
        KEY => {
            let state = panel(hwnd);
            if let Some(old) = state.hover.take() {
                InvalidateRect(state.buttons[old], null(), 0);
            }
            navigate_panel(hwnd, wp as u16, (lp as usize).saturating_sub(100));
            0
        }
        HOVER => {
            let state = panel(hwnd);
            let next = wp.checked_sub(100).filter(|&i| i < state.items.len());
            if state.hover != next {
                let old = state.hover;
                state.hover = next;
                if let Some(i) = old {
                    InvalidateRect(state.buttons[i], null(), 0);
                }
                if let Some(i) = next {
                    InvalidateRect(state.buttons[i], null(), 0);
                    state.selected = i;
                    SetTimer(hwnd, 2, 220, None);
                }
            }
            0
        }
        WM_TIMER if wp == 2 => {
            KillTimer(hwnd, 2);
            let state = panel(hwnd);
            if let Some(i) = state.hover {
                if !state.items[i].children.is_empty() {
                    invoke(hwnd, i, false);
                } else {
                    close_panels(state.depth + 1);
                }
            }
            0
        }
        WM_ACTIVATE if wp & 0xffff == WA_INACTIVE as usize => {
            let other = lp as HWND;
            if !PANELS.with_borrow(|p| p.contains(&other))
                && !BARS.with_borrow(|b| b.contains(&other))
            {
                PostMessageW(state_host(), WM_CANCELMODE, 0, 0);
            }
            0
        }
        WM_CLOSE => {
            close_panels(0);
            0
        }
        WM_NCDESTROY => {
            let state = Box::from_raw(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Panel);
            DeleteObject(state.font);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn state_host() -> HWND {
    HOST.load(Ordering::Relaxed) as HWND
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
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
            surface::paint_background(hwnd);
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_DRAWITEM => {
            let draw = &*(lp as *const DRAWITEMSTRUCT);
            let state = bar(hwnd);
            let index = draw.CtlID.saturating_sub(100) as usize;
            if let Some(item) = state.triggers.get(index) {
                surface::draw_button(
                    draw,
                    &item.label,
                    state.fonts[if item.icon {
                        2
                    } else if item.bold {
                        1
                    } else {
                        0
                    }],
                    state.hover == index as i32 || OPEN.get() == Some((hwnd, index)),
                    None,
                );
            }
            1
        }
        WM_COMMAND => {
            if wp >> 16 == BN_CLICKED as usize {
                let index = (wp & 0xffff).saturating_sub(100);
                if OPEN.get() == Some((hwnd, index)) {
                    close_panels(0);
                } else {
                    open_trigger(hwnd, index, false);
                }
            }
            0
        }
        HOVER => {
            let state = bar(hwnd);
            let index = wp
                .checked_sub(100)
                .filter(|&i| i < state.buttons.len())
                .map_or(-1, |i| i as i32);
            if state.hover != index {
                let old = state.hover;
                state.hover = index;
                if old >= 0 {
                    InvalidateRect(state.buttons[old as usize], null(), 0);
                }
                if index >= 0 {
                    InvalidateRect(state.buttons[index as usize], null(), 0);
                    if !state.triggers[index as usize].items.is_empty()
                        && OPEN
                            .get()
                            .is_some_and(|(owner, i)| owner != hwnd || i != index as usize)
                    {
                        open_trigger(hwnd, index as usize, false);
                    }
                }
            }
            0
        }
        KEY => {
            let index = (lp as usize).saturating_sub(100);
            let count = bar(hwnd).buttons.len();
            if count == 0 {
                return 0;
            }
            match wp as u16 {
                VK_LEFT | VK_RIGHT | VK_TAB => {
                    let previous = wp as u16 == VK_LEFT
                        || (wp as u16 == VK_TAB && GetKeyState(VK_SHIFT as i32) < 0);
                    let next = (index as isize + if previous { -1 } else { 1 })
                        .rem_euclid(count as isize) as usize;
                    SetFocus(bar(hwnd).buttons[next]);
                }
                VK_DOWN | VK_RETURN | VK_SPACE => open_trigger(hwnd, index, true),
                VK_ESCAPE => {
                    close_panels(0);
                    SetForegroundWindow(SNAPSHOT.with_borrow(|s| s.target) as HWND);
                }
                _ => {}
            }
            0
        }
        WM_HOTKEY => {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetForegroundWindow(hwnd);
            if let Some(&b) = bar(hwnd).buttons.first() {
                SetFocus(b);
            }
            0
        }
        WM_CANCELMODE => {
            close_panels(0);
            0
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            close_panels(0);
            0
        }
        REFRESH => {
            close_panels(0);
            request(Work::Refresh(wp as isize));
            0
        }
        m if m == WM_APP + 7 => {
            request(Work::Status);
            0
        }
        m if m == WM_APP + 8 => {
            if OPEN.get().is_none() {
                BARS.with_borrow(|bars| {
                    for &bar in bars {
                        layout(bar);
                    }
                });
            } else {
                DIRTY.set(true);
            }
            0
        }
        UPDATED => {
            if let Some(update) = LATEST.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let changed = SNAPSHOT.with_borrow(|s| *s != update.snapshot);
                let same_app = SNAPSHOT.with_borrow(|s| {
                    s.target == update.snapshot.target && s.name == update.snapshot.name
                });
                let reopen = OPEN.get().filter(|&(owner, index)| {
                    same_app
                        && bar(owner)
                            .triggers
                            .get(index)
                            .is_some_and(|t| t.app_index.is_some())
                });
                let status_changed = STATUS.with_borrow(|s| *s != update.status);
                SNAPSHOT.with_borrow_mut(|s| *s = update.snapshot);
                STATUS.with_borrow_mut(|s| *s = update.status);
                // Preserve live popup HWNDs while status values change. Rebuild only closed bars.
                if changed || status_changed && OPEN.get().is_none() && !control::is_open() {
                    if changed {
                        close_panels(0);
                    }
                    BARS.with_borrow(|bars| {
                        for &bar in bars {
                            layout(bar);
                        }
                    });
                    if changed && let Some((owner, index)) = reopen {
                        open_trigger(owner, index, true);
                    }
                } else if status_changed {
                    DIRTY.set(true);
                    refresh_controls();
                }
                control::refresh();
            }
            0
        }
        WM_TIMER if wp == 1 => {
            let config = CONFIG.with_borrow(Clone::clone);
            let clock = platform::clock(config.seconds);
            let state = bar(hwnd);
            if state.clock != clock {
                state.clock = clock.clone();
                if let Some(index) = state.triggers.len().checked_sub(1) {
                    state.triggers[index].accessible = format!("{clock}, open Control Center");
                    SetWindowTextW(
                        state.buttons[index],
                        wide(&state.triggers[index].accessible).as_ptr(),
                    );
                    state.triggers[index].label = clock;
                    InvalidateRect(state.buttons[index], null(), 0);
                }
            }
            let fullscreen = SNAPSHOT.with_borrow(|s| s.fullscreen);
            let mut cursor = zeroed();
            GetCursorPos(&mut cursor);
            let over = cursor.x >= state.rect.left
                && cursor.x < state.rect.right
                && cursor.y >= state.rect.top
                && cursor.y < state.rect.bottom;
            let should_hide = (config.auto_hide || fullscreen)
                && !over
                && OPEN.get().is_none()
                && !control::is_open()
                && GetForegroundWindow() != hwnd;
            if should_hide != state.hidden {
                state.hidden = should_hide;
                position(hwnd);
            }
            0
        }
        WM_POWERBROADCAST => {
            request(Work::Action(
                Action::None,
                SNAPSHOT.with_borrow(|s| s.target),
            ));
            1
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            close_panels(0);
            surface::apply(hwnd, false);
            position(hwnd);
            layout(hwnd);
            0
        }
        WM_DPICHANGED => {
            close_panels(0);
            position(hwnd);
            layout(hwnd);
            0
        }
        WM_DISPLAYCHANGE => {
            SetTimer(state_host(), 3, 350, None);
            0
        }
        WM_TIMER if wp == 3 => {
            KillTimer(hwnd, 3);
            PostMessageW(hwnd, REBUILD, 0, 0);
            0
        }
        REBUILD => {
            rebuild_bars();
            0
        }
        APPBAR => {
            if wp == ABN_POSCHANGED as usize {
                position(hwnd);
            } else if wp == ABN_FULLSCREENAPP as usize {
                SetWindowPos(
                    hwnd,
                    if lp != 0 { HWND_BOTTOM } else { HWND_TOPMOST },
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            0
        }
        WM_ACTIVATE => {
            if bar(hwnd).registered {
                let mut d: APPBARDATA = zeroed();
                d.cbSize = size_of::<APPBARDATA>() as u32;
                d.hWnd = hwnd;
                SHAppBarMessage(ABM_ACTIVATE, &mut d);
            }
            0
        }
        WM_CLOSE => {
            close_panels(0);
            let bars = BARS.with_borrow_mut(std::mem::take);
            for bar in bars {
                DestroyWindow(bar);
            }
            PostQuitMessage(0);
            0
        }
        WM_DESTROY => {
            let state = bar(hwnd);
            if state.registered {
                let mut d: APPBARDATA = zeroed();
                d.cbSize = size_of::<APPBARDATA>() as u32;
                d.hWnd = hwnd;
                SHAppBarMessage(ABM_REMOVE, &mut d);
            }
            UnregisterHotKey(hwnd, 1);
            0
        }
        WM_NCDESTROY => {
            let state = Box::from_raw(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Bar);
            for font in state.fonts {
                DeleteObject(font);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> i32 {
    (*(data as *mut Vec<HMONITOR>)).push(monitor);
    1
}

unsafe fn create_bar(monitor: HMONITOR) -> HWND {
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    let state = Box::new(Bar {
        monitor,
        dpi: 96,
        rect: info.rcMonitor,
        buttons: vec![],
        triggers: vec![],
        fonts: [
            surface::font(96, false, false),
            surface::font(96, true, false),
            surface::font(96, false, true),
        ],
        registered: false,
        hidden: false,
        clock: String::new(),
        hover: -1,
    });
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        wide(CLASS).as_ptr(),
        wide("WinRust Menu Bar").as_ptr(),
        WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_CLIPCHILDREN,
        info.rcMonitor.left,
        info.rcMonitor.top,
        info.rcMonitor.right - info.rcMonitor.left,
        28,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        Box::into_raw(state).cast(),
    );
    if hwnd.is_null() {
        return hwnd;
    }
    let mut data: APPBARDATA = zeroed();
    data.cbSize = size_of::<APPBARDATA>() as u32;
    data.hWnd = hwnd;
    data.uCallbackMessage = APPBAR;
    bar(hwnd).registered = SHAppBarMessage(ABM_NEW, &mut data) != 0;
    surface::apply(hwnd, false);
    position(hwnd);
    layout(hwnd);
    SetTimer(hwnd, 1, 1000, None);
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    hwnd
}

unsafe fn rebuild_bars() {
    close_panels(0);
    let mut monitors = vec![];
    EnumDisplayMonitors(
        null_mut(),
        null(),
        Some(monitor_callback),
        (&mut monitors as *mut Vec<HMONITOR>) as isize,
    );
    if CONFIG.with_borrow(|c| c.primary_only) {
        monitors.retain(|&m| {
            let mut i: MONITORINFO = zeroed();
            i.cbSize = size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(m, &mut i);
            i.dwFlags & MONITORINFOF_PRIMARY != 0
        });
    }
    let old = BARS.with_borrow_mut(std::mem::take);
    for hwnd in old {
        DestroyWindow(hwnd);
    }
    let mut windows = vec![];
    for monitor in monitors {
        let hwnd = create_bar(monitor);
        if !hwnd.is_null() {
            windows.push(hwnd);
        }
    }
    if let Some(&first) = windows.first() {
        HOST.store(first as isize, Ordering::Relaxed);
        RegisterHotKey(
            first,
            1,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_F10 as u32,
        );
    }
    BARS.with_borrow_mut(|bars| *bars = windows);
}

fn main() {
    unsafe {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|s| s == "--quit") {
            let hwnd = FindWindowW(wide(CLASS).as_ptr(), null());
            if !hwnd.is_null() {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            return;
        }
        let mutex = windows_sys::Win32::System::Threading::CreateMutexW(
            null(),
            0,
            wide("Local\\WinRust.MenuBar.Singleton").as_ptr(),
        );
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if !mutex.is_null() {
                CloseHandle(mutex);
            }
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        BufferedPaintInit();
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        });
        let mut config = Config::load();
        config.seconds |= args.iter().any(|s| s == "--seconds");
        config.auto_hide |= args.iter().any(|s| s == "--auto-hide");
        config.primary_only |= args.iter().any(|s| s == "--primary-only");
        CONFIG.with_borrow_mut(|c| *c = config);
        for (name, proc) in [
            (
                CLASS,
                Some(bar_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
            ),
            (
                POPUP,
                Some(popup_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
            ),
        ] {
            let name = wide(name);
            let class = WNDCLASSW {
                lpfnWndProc: proc,
                hInstance: GetModuleHandleW(null()),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: name.as_ptr(),
                ..zeroed()
            };
            RegisterClassW(&class);
        }
        rebuild_bars();
        if state_host().is_null() {
            MessageBoxW(
                null_mut(),
                wide("Could not create the menu bar window.").as_ptr(),
                wide("WinRust Menu Bar").as_ptr(),
                MB_ICONERROR,
            );
            return;
        }
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            null_mut(),
            Some(foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        start_worker();
        request(Work::Refresh(GetForegroundWindow() as isize));
        let mut message = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if !hook.is_null() {
            UnhookWinEvent(hook);
        }
        SENDER.with_borrow_mut(|sender| *sender = None);
        BufferedPaintUnInit();
        if !mutex.is_null() {
            CloseHandle(mutex);
        }
    }
}
