# Menu bar design

The user-provided macOS Tahoe menu-bar specification is the reference. Mode: Operate. Extend WinRust with a separate native component, preserving existing shell components.

Use a quiet, full-width top-edge Mica surface, 28 logical pixels tall (24-pixel content with usable targets), DPI scaled from monitor geometry. Segoe UI at 13 logical pixels is the Windows system-font equivalent; the active app uses semibold. Icons use Segoe Fluent Icons. System appearance determines light/dark material and text. DWM controls material fallback, transparency and accessibility policy.

The user confirmed both standard Mica and Acrylic as settings options. Mica remains the default; persist the choice and apply it to all bar and popup windows. Status artwork is 13 logical pixels within the original 32-pixel targets. The user's requested padding means smaller artwork within targets, not wider spacing between icons.

Application menus occupy the left; essential system controls and a locale-aware clock anchor the right. Overflow moves whole menus into More. Popups are rounded Mica windows with 28-pixel rows, real disabled/checked states, submenus and shortcut labels. Native buttons supply accessible names, roles and focus. No idle motion. Hover switches open menus; Escape unwinds submenus; arrows navigate and Enter activates.

Control Center contains real network and audio information, volume actions and links to Windows detail controls. Windows Quick Settings, Search, Project and Notification Center are opened through their existing shell shortcuts. Do not invent unavailable hardware state or application commands.
