# Validation — November 20, 2026

- `cargo clippy -p menubar --all-targets -- -D warnings`: passed.
- `cargo test -p menubar`: 3 passed. Includes native Win32 menu extraction, disabled/checked/nested state, accelerator text, command dispatch, and overflow behavior.
- `scripts/build-all.ps1`: builds every plugin, the new payload entry, and the complete installer.
- `cargo test --workspace`: failed in the existing `rtaskmgr-win32` GPU test because it explicitly requires an NVIDIA adapter; the development machine has AMD adapters. That test was left unchanged.
- `cargo test --workspace -- --skip gpu::tests::real_adapters_have_memory_directx_location_and_temperature`: 143 passed, 5 ignored, 1 filtered out.
- DWM diagnostics reported successful backdrop application and query. Desktop captures confirmed bars on two monitors, including a monitor above the primary, and a working foreground-name update. Visual iteration corrected the system mark and icon sizing.

Interactive automation was interrupted by physical desktop input (`CONTROL_PREEMPTED`, then `USER_CONTROL`). Complete keyboard, submenu, material-selection, and slider interaction verification remains a manual check; the example fixture is provided for it. No system radio, volume, power, or shell replacement was changed by automated interaction testing.

The implementation's platform boundaries and remaining specification gaps are listed in README.md. Build/test success does not imply every optional macOS integration has a Windows equivalent.
