# Validation — November 20, 2026

- `cargo clippy -p menubar --all-targets -- -D warnings`: passed.
- `cargo test -p menubar`: 3 passed. Includes native Win32 menu extraction, disabled/checked/nested state, accelerator text, command dispatch, and overflow behavior.
- `scripts/build-all.ps1`: builds every plugin, the new payload entry, and the complete installer.
- `cargo test --workspace`: failed in the existing `rtaskmgr-win32` GPU test because it explicitly requires an NVIDIA adapter; the development machine has AMD adapters. That test was left unchanged.
- `cargo test --workspace -- --skip gpu::tests::real_adapters_have_memory_directx_location_and_temperature`: 143 passed, 5 ignored, 1 filtered out.
- DWM diagnostics reported successful backdrop application and query. Desktop captures confirmed bars on two monitors, including a monitor above the primary, and a working foreground-name update. Visual iteration corrected the system mark and icon sizing.

Interactive automation was interrupted by physical desktop input (`CONTROL_PREEMPTED`, then `USER_CONTROL`). Complete keyboard, submenu, material-selection, and slider interaction verification remains a manual check; the example fixture is provided for it. No system radio, volume, power, or shell replacement was changed by automated interaction testing.

The implementation's platform boundaries and remaining specification gaps are listed in README.md. Build/test success does not imply every optional macOS integration has a Windows equivalent.

## Context menu and Control Center follow-up

Clippy and the three module tests pass after the follow-up. Live desktop clicks verified that the time button opens separate Acrylic tile windows and opening the Windows menu dismisses those tiles. The Windows menu's icon gutter, shortcut/chevron layout, borderless white selection, and regular-weight app name were visually inspected. Native slider input/accessibility remains intact while custom painting removes the initially observed opaque slider backgrounds. The final slider artwork was inspected in the running panel.

The test machine reported no active Windows media session and no supported DDC/CI brightness for the selected display; the UI showed those unavailable states and disabled the corresponding controls. Actual radio changes, audio changes, DDC/CI writes, media transport, and the complete keyboard flow were not exercised on the user's hardware. These checks remain manual.

## Mica and icon-only follow-up

The Control Center now retains visible text only for Wi-Fi and Focus. Other tiles use centered icons; accessibility names, media metadata and native slider values remain available. A live desktop capture verified the revised layout. DWM diagnostics reported successful standard Mica (`type=2`) application/query for all fourteen tiles. Context menus continue to use Acrylic. The installer build, Clippy and all three module tests passed.

## Lower borders, softer material and active backgrounds

The bar reserves one physical pixel beneath its buttons for a lower hairline, and each tile paints a matching lower edge. A neutral premultiplied wash reduces wallpaper tint consistently across parent and child surfaces. The footer now reads Open Settings. Actual radio states drive Wi-Fi/Bluetooth accent backgrounds; guarded Focus-session detection and media playback state drive their corresponding active backgrounds. Continuous-value controls and action launchers do not invent an on/off state.

A live capture verified the lower edges, text footer, and accent-colored Wi-Fi and Bluetooth tiles while the other tiles remained neutral. No radios or system settings were changed to create this proof. Focus activation and media playback transitions were not exercised. Module tests and Clippy passed.
