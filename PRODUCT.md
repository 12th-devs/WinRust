# WinRust Menu Bar

## Platform
Native Windows desktop, Windows 11 22H2 or newer for Mica; opaque OS fallback on older versions.

## Stack
Rust, Win32 windows and accessible native controls, DWM system material. No web runtime.

## Purpose
A standalone WinRust module that adds a persistent macOS-style top menu bar. It is separate from the existing WinRust bottom bar and can coexist with the Windows taskbar.

## Capabilities and constraints
The user confirmed real Win32 application menus with window-action fallback for applications without exposed menus, and a Windows symbol for the system menu. The supplied macOS menu-bar specification is the visual and interaction reference. Windows-native shortcuts and system panels preserve platform functionality. Arbitrary custom application menus cannot be universally extracted.

Follow-up requirements: Windows-style Acrylic context menus, translucent-white borderless selection, regular-weight foreground app name, and a clock-triggered Control Center with separate native material surfaces for each tile. Control Center tiles use Mica and show labels only for Wi-Fi and Focus; other controls retain accessible names. Bar Mica/Acrylic preference remains independent of Acrylic context menus and Mica Control Center tiles.

## Success
Responsive native top-edge surface with Mica, foreground application tracking, actionable menus, system information, keyboard access, DPI and monitor support, and reversible standalone installation.
