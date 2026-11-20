use crate::model::{Action, Item, menu_label};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA,
    Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Globalization::*,
    NetworkManagement::WiFi::*,
    System::{Power::*, Registry::*, SystemInformation::*, Threading::*},
    UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

#[windows_core::implement(IAudioEndpointVolumeCallback)]
struct VolumeEvents;
impl IAudioEndpointVolumeCallback_Impl for VolumeEvents_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        crate::notify_status();
        Ok(())
    }
}

/// Lifetimes match the worker. Callbacks post messages; they never touch UI state.
pub struct Subscriptions {
    wlan: HANDLE,
    audio: Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
}
impl Subscriptions {
    pub fn new() -> Self {
        unsafe {
            let mut wlan = null_mut();
            let mut version = 0;
            if WlanOpenHandle(2, null(), &mut version, &mut wlan) == 0 {
                WlanRegisterNotification(
                    wlan,
                    WLAN_NOTIFICATION_SOURCE_ACM,
                    1,
                    Some(wlan_changed),
                    null(),
                    null(),
                    null_mut(),
                );
            }
            let audio = (|| -> windows::core::Result<_> {
                use windows::Win32::{
                    Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender},
                    System::Com::*,
                };
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let endpoint: IAudioEndpointVolume = enumerator
                    .GetDefaultAudioEndpoint(eRender, eConsole)?
                    .Activate(CLSCTX_ALL, None)?;
                let callback: IAudioEndpointVolumeCallback = VolumeEvents.into();
                endpoint.RegisterControlChangeNotify(&callback)?;
                Ok((endpoint, callback))
            })()
            .ok();
            Self { wlan, audio }
        }
    }
}
unsafe extern "system" fn wlan_changed(_: *mut L2_NOTIFICATION_DATA, _: *mut std::ffi::c_void) {
    crate::notify_status();
}
impl Drop for Subscriptions {
    fn drop(&mut self) {
        unsafe {
            if !self.wlan.is_null() {
                WlanRegisterNotification(
                    self.wlan,
                    WLAN_NOTIFICATION_SOURCE_NONE,
                    1,
                    None,
                    null(),
                    null(),
                    null_mut(),
                );
                WlanCloseHandle(self.wlan, null());
            }
            if let Some((endpoint, callback)) = &self.audio {
                let _ = endpoint.UnregisterControlChangeNotify(callback);
            }
        }
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
pub fn from_wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

pub fn setting(name: &str, default: u32) -> u32 {
    unsafe {
        let mut value = default;
        let mut len = 4;
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut len,
        );
        value
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub target: isize,
    pub name: String,
    pub menus: Vec<Item>,
    pub fullscreen: bool,
}

/// Called on a worker, never on the window thread. Dynamic menu initialization is
/// bounded with SMTO_ABORTIFHUNG; unsupported owner-drawn entries are not guessed.
pub fn snapshot(target: isize) -> Snapshot {
    unsafe {
        let hwnd = target as HWND;
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mut path = [0u16; 32768];
        let mut len = path.len() as u32;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        let mut name = String::from("Desktop");
        if !process.is_null() {
            if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) != 0 {
                let full = String::from_utf16_lossy(&path[..len as usize]);
                name = std::path::Path::new(&full)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if name.eq_ignore_ascii_case("explorer") {
                    name = "Explorer".into();
                }
            }
            CloseHandle(process);
        }
        let mut budget = 256;
        let menu = GetMenu(hwnd);
        let menus = if menu.is_null() {
            vec![]
        } else {
            read_menu(menu, 0, &mut budget)
        };
        let mut rect = zeroed();
        GetWindowRect(hwnd, &mut rect);
        let mon = windows_sys::Win32::Graphics::Gdi::MonitorFromWindow(hwnd, 2);
        let mut info: windows_sys::Win32::Graphics::Gdi::MONITORINFO = zeroed();
        info.cbSize = size_of::<windows_sys::Win32::Graphics::Gdi::MONITORINFO>() as u32;
        windows_sys::Win32::Graphics::Gdi::GetMonitorInfoW(mon, &mut info);
        let m = info.rcMonitor;
        let fullscreen = name != "Explorer"
            && name != "Desktop"
            && rect.left <= m.left
            && rect.top <= m.top
            && rect.right >= m.right
            && rect.bottom >= m.bottom;
        Snapshot {
            target,
            name,
            menus,
            fullscreen,
        }
    }
}

unsafe fn read_menu(menu: HMENU, depth: usize, budget: &mut usize) -> Vec<Item> {
    if depth > 6 || *budget == 0 {
        return vec![];
    }
    let mut out = vec![];
    for index in 0..GetMenuItemCount(menu).clamp(0, 64) as u32 {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let mut label = [0u16; 512];
        let mut item: MENUITEMINFOW = zeroed();
        item.cbSize = size_of::<MENUITEMINFOW>() as u32;
        item.fMask = MIIM_STRING | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_FTYPE;
        item.dwTypeData = label.as_mut_ptr();
        item.cch = label.len() as u32;
        if GetMenuItemInfoW(menu, index, 1, &mut item) == 0 {
            continue;
        }
        if item.fType & MFT_SEPARATOR != 0 {
            out.push(Item::separator());
            continue;
        }
        let text = menu_label(&from_wide(&label));
        if text.is_empty() || item.fType & MFT_OWNERDRAW != 0 {
            continue;
        }
        let children = if item.hSubMenu.is_null() {
            vec![]
        } else {
            read_menu(item.hSubMenu, depth + 1, budget)
        };
        out.push(Item {
            label: text,
            enabled: item.fState & (MFS_DISABLED | MFS_GRAYED) == 0,
            checked: item.fState & MFS_CHECKED != 0,
            action: Action::Command(item.wID),
            children,
        });
    }
    out
}

pub fn initialize_menu(target: isize, index: usize) {
    unsafe {
        let hwnd = target as HWND;
        let menu = GetMenu(hwnd);
        if !menu.is_null() {
            let sub = GetSubMenu(menu, index as i32);
            let mut result = 0;
            SendMessageTimeoutW(
                hwnd,
                WM_INITMENU,
                menu as usize,
                0,
                SMTO_ABORTIFHUNG,
                80,
                &mut result,
            );
            if !sub.is_null() {
                SendMessageTimeoutW(
                    hwnd,
                    WM_INITMENUPOPUP,
                    sub as usize,
                    index as isize,
                    SMTO_ABORTIFHUNG,
                    80,
                    &mut result,
                );
            }
        }
    }
}

pub fn clock(seconds: bool) -> String {
    unsafe {
        let mut time = zeroed();
        GetLocalTime(&mut time);
        let mut date = [0u16; 128];
        let mut text = [0u16; 128];
        GetDateFormatW(
            LOCALE_USER_DEFAULT,
            0,
            &time,
            wide("ddd").as_ptr(),
            date.as_mut_ptr(),
            date.len() as i32,
        );
        GetTimeFormatW(
            LOCALE_USER_DEFAULT,
            if seconds { 0 } else { TIME_NOSECONDS },
            &time,
            null(),
            text.as_mut_ptr(),
            text.len() as i32,
        );
        format!("{}  {}", from_wide(&date), from_wide(&text))
    }
}

pub fn battery() -> Option<(u8, bool)> {
    unsafe {
        let mut p = zeroed();
        if GetSystemPowerStatus(&mut p) == 0 || p.BatteryFlag == 128 || p.BatteryLifePercent > 100 {
            None
        } else {
            Some((p.BatteryLifePercent, p.ACLineStatus == 1))
        }
    }
}

pub fn wifi() -> String {
    unsafe {
        let mut client = null_mut();
        let mut version = 0;
        if WlanOpenHandle(2, null(), &mut version, &mut client) != 0 {
            return "Wi-Fi unavailable".into();
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
        let mut status = "Wi-Fi disconnected".to_string();
        if WlanEnumInterfaces(client, null(), &mut list) == 0 && !list.is_null() {
            let entries = std::slice::from_raw_parts(
                (*list).InterfaceInfo.as_ptr(),
                (*list).dwNumberOfItems.min(32) as usize,
            );
            for interface in entries {
                if interface.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut data = null_mut();
                let mut size = 0;
                let mut kind = 0;
                if WlanQueryInterface(
                    client,
                    &interface.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    null(),
                    &mut size,
                    &mut data,
                    &mut kind,
                ) == 0
                    && !data.is_null()
                {
                    let attributes = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let ssid = &attributes.wlanAssociationAttributes.dot11Ssid;
                    let bytes = &ssid.ucSSID[..(ssid.uSSIDLength as usize).min(32)];
                    status = format!("Wi-Fi: {}", String::from_utf8_lossy(bytes));
                    WlanFreeMemory(data);
                }
            }
            WlanFreeMemory(list.cast());
        }
        WlanCloseHandle(client, null());
        status
    }
}

pub fn bluetooth() -> String {
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    let result = (|| -> windows::core::Result<bool> {
        let radios = Radio::GetRadiosAsync()?.join()?;
        for radio in radios {
            if radio.Kind()? == RadioKind::Bluetooth {
                return Ok(radio.State()? == RadioState::On);
            }
        }
        Ok(false)
    })();
    match result {
        Ok(true) => "Bluetooth on",
        Ok(false) => "Bluetooth off / unavailable",
        Err(_) => "Bluetooth unavailable",
    }
    .into()
}

pub fn volume(change: Option<i32>, mute: bool) -> Option<(i32, bool)> {
    use windows::Win32::{
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole,
            eRender,
        },
        System::Com::*,
    };
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let endpoint: IAudioEndpointVolume = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()?
            .Activate(CLSCTX_ALL, None)
            .ok()?;
        if let Some(delta) = change {
            let value =
                (endpoint.GetMasterVolumeLevelScalar().ok()? + delta as f32 / 100.).clamp(0., 1.);
            endpoint.SetMasterVolumeLevelScalar(value, null()).ok()?;
        }
        if mute {
            endpoint
                .SetMute(!endpoint.GetMute().ok()?.as_bool(), null())
                .ok()?;
        }
        Some((
            (endpoint.GetMasterVolumeLevelScalar().ok()? * 100.).round() as i32,
            endpoint.GetMute().ok()?.as_bool(),
        ))
    }
}

pub fn set_volume(level: i32) {
    if let Some((current, _)) = volume(None, false) {
        volume(Some(level.clamp(0, 100) - current), false);
    }
}

pub fn execute(action: &Action, target: isize) {
    unsafe {
        match action {
            Action::Command(id) | Action::System(id) => {
                let hwnd = target as HWND;
                if IsWindow(hwnd) != 0 {
                    SetForegroundWindow(hwnd);
                    PostMessageW(
                        hwnd,
                        if matches!(action, Action::System(_)) {
                            WM_SYSCOMMAND
                        } else {
                            WM_COMMAND
                        },
                        *id as usize,
                        0,
                    );
                }
            }
            Action::Uri(uri) => {
                ShellExecuteW(
                    null_mut(),
                    wide("open").as_ptr(),
                    wide(uri).as_ptr(),
                    null(),
                    null(),
                    SW_SHOWNORMAL,
                );
            }
            Action::Keys(keys) => {
                let mut input = vec![];
                for (&key, flags) in keys
                    .iter()
                    .map(|k| (k, 0))
                    .chain(keys.iter().rev().map(|k| (k, KEYEVENTF_KEYUP)))
                {
                    input.push(INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: key,
                                wScan: 0,
                                dwFlags: flags,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
                SendInput(
                    input.len() as u32,
                    input.as_ptr(),
                    size_of::<INPUT>() as i32,
                );
            }
            Action::Volume(delta) => {
                volume(Some(*delta), false);
            }
            Action::VolumeSet(level) => set_volume(*level),
            Action::Mute => {
                volume(None, true);
            }
            Action::Lock => {
                ShellExecuteW(
                    null_mut(),
                    wide("open").as_ptr(),
                    wide("rundll32.exe").as_ptr(),
                    wide("user32.dll,LockWorkStation").as_ptr(),
                    null(),
                    SW_SHOWNORMAL,
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COMMAND: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "system" fn test_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_COMMAND {
            COMMAND.store(wp, Ordering::SeqCst);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    #[test]
    fn reads_native_menu_states_and_dispatches_a_real_command() {
        unsafe {
            use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
            let name = wide("WinRust.MenuAdapter.Test");
            let class = WNDCLASSW {
                lpfnWndProc: Some(test_proc),
                hInstance: GetModuleHandleW(null()),
                lpszClassName: name.as_ptr(),
                ..zeroed()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let root = CreateMenu();
            let file = CreatePopupMenu();
            let sub = CreatePopupMenu();
            AppendMenuW(file, MF_STRING, 42, wide("&Run\tCtrl+R").as_ptr());
            AppendMenuW(file, MF_GRAYED, 43, wide("Disabled").as_ptr());
            AppendMenuW(sub, MF_CHECKED, 44, wide("Checked").as_ptr());
            AppendMenuW(file, MF_POPUP, sub as usize, wide("Nested").as_ptr());
            AppendMenuW(root, MF_POPUP, file as usize, wide("&File").as_ptr());
            let hwnd = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                300,
                200,
                null_mut(),
                root,
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!hwnd.is_null());
            let state = snapshot(hwnd as isize);
            assert_eq!(state.menus[0].label, "File");
            let items = &state.menus[0].children;
            assert_eq!(items[0].label, "Run\tCtrl+R");
            assert!(!items[1].enabled);
            assert!(items[2].children[0].checked);
            execute(&items[0].action, hwnd as isize);
            let mut message = zeroed();
            while PeekMessageW(&mut message, hwnd, WM_COMMAND, WM_COMMAND, PM_REMOVE) != 0 {
                DispatchMessageW(&message);
            }
            assert_eq!(COMMAND.load(Ordering::SeqCst), 42);
            DestroyWindow(hwnd);
            UnregisterClassW(name.as_ptr(), GetModuleHandleW(null()));
        }
    }
}
