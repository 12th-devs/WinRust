# Plugin Launcher: Roadmap

(Русская версия: [ROADMAP.ru.md](ROADMAP.ru.md))

## Done
- **English by default + a language switch**: every program (installer, topbar, task manager, disk cleaner) is in English; a Русский/English button in the installer window and a "Language" item in the bar's menu switch it. The choice is stored in `%ProgramData%\WinRust\lang.txt` and read when a program starts. Texts are written as pairs in the code (`t!("русский", "english")`, `tf!` for format strings; `src/i18n.rs` is identical in each crate); unit tests run in Russian. Only built and unit-tested, the windows were not walked through by hand.
- A Win32/GDI window in the rtaskmgr-win32 style (dark theme, plugin cards).
- Plugins embedded into one `plugin-launcher.exe` (unpacked into Program Files, not %TEMP%), static CRT.
- **rust-search**: an NTFS search service + Alt+Space; disables WSearch and the taskbar search box.
- **rust-search, right-click on a result**: rename (inline, in the input line), move to recycle bin, "delete permanently + clean the registry". Deletion runs in three steps inside the card: scan (files, size, list of registry entries) → confirmation ("Cancel" by default) → delete files (clears read-only, never follows a junction/symlink); the registry is touched only if every file is gone, and is saved first to `%LOCALAPPDATA%\rs-search\registry-backups\*.reg`. An Uninstall/App Paths entry is removed whole only if the program itself lived inside the deleted folder. Sources: `rust-search/src/bin/rs-launcher.rs`, `src/regsweep.rs`; checked with tests and an end-to-end GUI run.
- **rtaskmgr-win32**: replaces Task Manager through an IFEO Debugger (`taskmgr.exe` itself is untouched). On a panic it writes `%APPDATA%\rtaskmgr\crash.log` and opens the real Taskmgr bypassing IFEO (started under a debugger that detaches at once; through UAC without admin rights). Understands `taskmgr /startup`, has "Restart as administrator". No clippy warnings.
- **topbar**: a bottom bar (AppBar) in the Windows 11 style instead of the taskbar: logo (opens Start), window buttons with icons (left click: minimize/activate, right/middle click: close), date and clock. It hides the native taskbar and sets it to auto-hide; uninstall brings it back (`topbar.exe --restore`). Was only built at that point, not run on a live system.
- **topbar + PowerShell profile**: installing topbar writes a block between the `# >>> topbar >>>` / `# <<< topbar <<<` markers into the profile (`src/profile.ps1`: zoxide, `grep`=rg, `ll`=eza, `cat`=bat). Written to Windows PowerShell 5.1 and to pwsh 7 if present; reinstalling does not duplicate the block, other profile lines are untouched, uninstalling topbar removes the block (and the file if nothing else was in it). Checked on a temporary profile; not checked in pwsh or by an end-to-end install.
- **Command line**: `plugin-launcher list | install <plugin> --yes | uninstall <plugin> | rollback --yes`. Without `--yes`, install only prints the warning. Exit codes 0/1/2. Needs an administrator terminal. The window and the CLI call the same functions. Only the commands that do not change the system were checked.
- **Plugins as data**: a `PLUGINS` entry includes the icon (`glyph`); a new plugin is added with one entry, and the window and CLI pick it up on their own.
- **topbar safety net**: `reshell` (always running with the bar) checks every 5 s that topbar is not running while the native taskbar is hidden; twice in a row (~10 s) it runs `topbar.exe --restore`. It does not restart the bar (to avoid a loop when it crashes on start). Covered by a unit test of the condition; the end-to-end scenario (kill topbar) was not checked.
- **More robust installer**: a failed install rolls its changes back (not when updating an existing copy); the previous WSearch and search-box values are kept in `HKLM\SOFTWARE\plugin-launcher\rust-search` and restored on uninstall; non-fatal errors (shortcut, scheduled task, restore point, profile) are counted and written to `%ProgramData%\plugin-launcher\install.log`; only what a plugin needs is unpacked. Only built, not run.
- **Update without rebuilding the launcher**: exes from `payload\` next to the launcher (or `..\..\payload` in the cargo layout) are read from disk instead of the embedded ones. `plugin-launcher update <plugin>` and `scripts\update-plugin.ps1 <plugin>` (build the plugin, put it into payload, update). The embedded copies stay as a fallback; releases still use `build-all.ps1`.
- A restore point before install, a "Roll everything back" button.
- Sources of all projects are collected in `plugins/`.

## Next (in order)
1. **Test on a clean Windows 11** (VM): install, uninstall, rollback, reboot. Nothing has been checked on another machine so far.
3. **chrome-rs**: a real install instead of the placeholder (once the browser is ready; see `plugins/chrome-rs/ROADMAP.md`).
4. **PowerShell profile**: optionally install rg/fd/bat/eza/zoxide through winget with a separate checkbox in the topbar settings (the block is silent without them today); check the pwsh 7 write on a machine that has it.
5. **Releases**: GitHub Releases (first one is out), SHA-256 next to it, version in the window, update check.
6. **Code signing** (removes SmartScreen and part of the antivirus false positives).
7. **Win+S / Start button**: open rust-search instead of Windows Search (risky, a separate decision).
8. **Non-NTFS drives** in rust-search (skipped today).
9. Tests for the install logic (sandbox/VM run), CI build. First check `install`/`uninstall`/`rollback` from the CLI on a VM: they were not run in the original session.

## Known limitations
- System components are not removed but disabled or bypassed (reversibly). Fully deleting `taskmgr.exe` and Windows Search is not possible without breaking system protection.
- Administrator rights are required.
- Win+S and Start still open Windows Search.

- **wiztree / photoview / debloat** (new, separate cargo projects; wiztree and photoview are in the launcher window, debloat only from the command line: `install debloat --yes`, `uninstall debloat`).
  - **wiztree**: a disk-usage treemap from the MFT, a GDI window; reads the MFT with the same code as rs-du (`plugins/rs-du/src/mft.rs`, wiztree pulls it in through `#[path]`). Click a folder: go in, right-click/Backspace: up, click a file: show in Explorer, Tab: next drive, F5: rescan. Restarts itself with UAC. Only the treemap layout is tested; not run on a live disk.
  - **photoview**: a WIC photo viewer (jpg/png/bmp/gif/tiff/webp/heic/avif if the codec is present), EXIF rotation, folder browsing, wheel zoom, Del to the recycle bin. Install adds "Open with" and an entry in "Default apps"; it does not become the default. Checked by launching on a system image; EXIF orientations 5 and 7 were not checked.
  - **debloat**: services (DiagTrack, maps, error reporting, etc.) and registry policies (Copilot, Recall, ads, Edge background). The old value of each change is written to `%ProgramData%\winrust-debloat\undo.txt` before the change, `--undo` reverts. The dry run (without `--apply`) was checked on a live system, `--apply`/`--undo` were not.
