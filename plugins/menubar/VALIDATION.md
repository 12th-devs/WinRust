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
