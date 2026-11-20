# WinRust Menu Bar

A separate Rust module for a macOS-style top menu bar on Windows. It coexists with the Windows taskbar or WinRust's `topbar` bottom bar. It does not hide Explorer, replace the shell, install hotkeys from ReShell, or change Windows services.

## Run

```powershell
cargo build -p menubar --release
.\target\release\menubar.exe
```

Windows 11 22H2+ provides native **Mica** and **Acrylic** through `DWMWA_SYSTEMBACKDROP_TYPE`. Choose **System menu → Menu Bar Settings → Background material**. Mica is the default and subtly incorporates the wallpaper; Acrylic provides visible frosted background blur. The choice persists between launches and applies to the bar and its popups. Windows controls power, contrast, and transparency fallbacks.

The bar registers an AppBar at the top of each monitor. Maximized applications respect its space. DPI and display-change notifications update the geometry; the bottom taskbar remains independent. Use `--primary-only` for one display.

## Interaction

- Click the Windows mark for system actions, preferences, or Quit Menu Bar.
- The active application's executable name appears in semibold. Its menu provides window actions.
- Apps exposing a standard Win32 `HMENU` also expose their real menu titles, nested commands, checked/disabled states, and existing shortcut text. Custom menus in Electron, UWP, browser chrome, and owner-drawn apps are not universally exposed; no synthetic File/Edit menus are inserted.
- Open menus switch on hover. Submenus open after a short delay. Outside clicks and Escape dismiss them. Arrow keys, Home/End, Enter, Space, Tab, and first-character navigation work in menus. Long menus scroll.
- **Ctrl+Alt+F10** focuses the bar. Use left/right or Tab to focus a trigger, then Down/Enter to open it. Application keyboard shortcuts remain owned by the application and retain Windows labels.
- Control Center provides real Wi-Fi/Bluetooth status, audio volume/mute, a native volume slider, and links to display, Focus, sound devices, accessibility and system Quick Settings. Wi-Fi/Bluetooth device management stays in Windows Settings. Search, notifications, and mirroring open Windows' existing interfaces.
- The clock uses the Windows locale, time zone and 12/24-hour preferences. Seconds, optional indicators, and auto-hide are configurable in **System menu → Menu Bar Settings**.
- Status icons are 13 logical pixels inside their original 32-pixel hit areas; this leaves room around the artwork without increasing the gap between controls.

Options: `--seconds`, `--auto-hide`, `--primary-only`, `--quit`, `--diagnostics`.

Preferences are stored at `%LOCALAPPDATA%\WinRust\menubar.cfg`. `--diagnostics` records only DWM material result codes and window style at `%LOCALAPPDATA%\WinRust\menubar-material.txt`.

## Install as a WinRust component

Build the usual WinRust installer with `scripts\build-all.ps1`, then select **menubar**, or run the elevated installer:

```text
plugin-launcher install menubar --yes
plugin-launcher update menubar
plugin-launcher uninstall menubar
```

The module is opt-in on fresh installs and retained when already installed. Installation adds `WinRust Menu Bar` to Start. Launch it there; installation does not automatically enable startup. To start at sign-in, put a shortcut to `menubar.exe` in your Windows Startup folder. Uninstall removes the executable and Start shortcut and asks the live process to release its AppBars. User preferences remain for reinstall.

## Integration and limits

The UI uses native accessible buttons and a native trackbar. Accessible labels identify system controls, checked entries, and submenus; disabled actions use the native disabled state. Custom popup containers currently expose window/button roles rather than a complete UIA Menu/MenuItem provider. There is no third-party tray-icon interception, custom notification history, media provider, or universal UI Automation menu scraping. Those remain Windows shell responsibilities.

Foreground changes use `SetWinEventHook`. Wi-Fi and master-volume changes use native callbacks; power changes use window notifications. Bluetooth and replacement audio endpoints have a 30-second fallback refresh. The clock wakes once per second but repaints only when its text changes. Status work and bounded foreign-menu initialization run on a worker, outside the UI thread. No idle animation runs. Auto-hide/fullscreen reveal is checked once per second; it is immediate and supports reduced motion by having no animation.

Native Mica is verified through DWM return codes and visual desktop checks. Hardware cutouts have no universal Windows safe-area API; Windows monitor/work-area geometry is honored, and narrow displays move application menus into More and optional indicators into Control Center. Physical cutout exclusion, exact screen-reader menu roles, modifier-drag status ordering, and animated edge reveal are future work.

## Verify

```powershell
cargo test -p menubar
cargo clippy -p menubar -- -D warnings
cargo build -p menubar --release
```

For a controlled real Win32 menu adapter check, run `cargo run -p menubar --example menu-fixture`. It creates only a test window; its commands update that window's title. Inspect File, its submenu, the disabled command, and checked state from the bar, then close the fixture.
