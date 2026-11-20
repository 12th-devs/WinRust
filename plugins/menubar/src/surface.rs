//! Native DWM Mica and alpha-correct themed text. No layered HWND or simulated blur.
use crate::platform::{setting, wide};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::*,
        WindowsAndMessaging::*,
    },
};

pub fn dark() -> bool {
    setting("AppsUseLightTheme", 1) == 0
}
pub fn high_contrast() -> bool {
    unsafe {
        let mut hc: HIGHCONTRASTW = zeroed();
        hc.cbSize = size_of::<HIGHCONTRASTW>() as u32;
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            (&mut hc as *mut HIGHCONTRASTW).cast(),
            0,
        ) != 0
            && hc.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}
pub unsafe fn apply(hwnd: HWND, rounded: bool) {
    let dark = dark() as i32;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
        (&dark as *const i32).cast(),
        4,
    );
    // Context menus always use native Desktop Acrylic, independently of the bar.
    let material: i32 = if rounded || crate::CONFIG.with_borrow(|c| c.acrylic) {
        3
    } else {
        2
    }; // Desktop Acrylic / standard Mica.
    let result = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE as u32,
        (&material as *const i32).cast(),
        4,
    );
    let corner: i32 = if rounded { 2 } else { 1 };
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
        (&corner as *const i32).cast(),
        4,
    );
    let border = if rounded {
        0xffff_ffffu32
    } else {
        0xffff_fffeu32
    }; // default menu outline / no bar outline
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR as u32,
        (&border as *const u32).cast(),
        4,
    );
    let extended = DwmExtendFrameIntoClientArea(
        hwnd,
        &MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        },
    );
    // Keep shell chrome visually active without stealing foreground focus.
    SendMessageW(hwnd, WM_NCACTIVATE, 1, 0);
    SetPropW(
        hwnd,
        wide("WinRust.Mica").as_ptr(),
        if result >= 0 && extended >= 0 {
            1usize as HANDLE
        } else {
            2usize as HANDLE
        },
    );
    if std::env::args().any(|arg| arg == "--diagnostics") {
        use std::io::Write;
        let path = crate::config::Config::path().with_file_name("menubar-material.txt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let mut actual = 0i32;
            let read = DwmGetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                (&mut actual as *mut i32).cast(),
                4,
            );
            let _ = writeln!(
                file,
                "backdrop HRESULT={result:#x}, extend HRESULT={extended:#x}, query HRESULT={read:#x}, type={actual}, dark={dark}, style={:#x}",
                GetWindowLongPtrW(hwnd, GWL_STYLE)
            );
        }
    }
}

pub unsafe fn font(dpi: u32, bold: bool, icon: bool) -> HFONT {
    sized_font(dpi, 13, bold, icon)
}

pub unsafe fn menu_font(dpi: u32) -> HFONT {
    sized_font(dpi, 14, false, false)
}
pub unsafe fn control_icon_font(dpi: u32) -> HFONT {
    sized_font(dpi, 22, false, true)
}

unsafe fn sized_font(dpi: u32, size: i32, bold: bool, icon: bool) -> HFONT {
    CreateFontW(
        -(size * dpi as i32 / 96),
        0,
        0,
        0,
        if bold { 600 } else { 400 },
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide(if icon {
            "Segoe Fluent Icons"
        } else {
            "Segoe UI"
        })
        .as_ptr(),
    )
}

pub unsafe fn measure(hwnd: HWND, text: &str, font: HFONT) -> i32 {
    let dc = GetDC(hwnd);
    let old = SelectObject(dc, font);
    let mut rect = zeroed();
    let value = wide(text);
    DrawTextW(
        dc,
        value.as_ptr(),
        -1,
        &mut rect,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(dc, old);
    ReleaseDC(hwnd, dc);
    rect.right
}

pub unsafe fn paint_background(hwnd: HWND) {
    let mut paint = zeroed();
    let dc = BeginPaint(hwnd, &mut paint);
    // Black on a glass-extended HWND is transparent to the system backdrop.
    let contrast = high_contrast();
    let brush = if contrast {
        GetSysColorBrush(COLOR_WINDOW)
    } else if GetPropW(hwnd, wide("WinRust.Mica").as_ptr()) as usize == 2 {
        CreateSolidBrush(if dark() { 0x00202020 } else { 0x00f3f3f3 })
    } else {
        GetStockObject(BLACK_BRUSH) as HBRUSH
    };
    FillRect(dc, &paint.rcPaint, brush);
    if !contrast && GetPropW(hwnd, wide("WinRust.Mica").as_ptr()) as usize == 2 {
        DeleteObject(brush);
    }
    EndPaint(hwnd, &paint);
}

pub unsafe fn draw_button(
    draw: &DRAWITEMSTRUCT,
    text: &str,
    font: HFONT,
    selected: bool,
    menu: Option<&crate::model::Item>,
) {
    let is_menu = menu.is_some();
    let rect = draw.rcItem;
    let mut dc = null_mut();
    let params = BP_PAINTPARAMS {
        cbSize: size_of::<BP_PAINTPARAMS>() as u32,
        dwFlags: BPPF_ERASE,
        prcExclude: null(),
        pBlendFunction: null(),
    };
    let buffer = BeginBufferedPaint(draw.hDC, &rect, BPBF_TOPDOWNDIB, &params, &mut dc);
    if buffer == 0 {
        return;
    }
    BufferedPaintClear(buffer, &rect);
    let disabled = draw.itemState & ODS_DISABLED != 0;
    let contrast = high_contrast();
    if contrast {
        FillRect(dc, &rect, GetSysColorBrush(COLOR_WINDOW));
        BufferedPaintSetAlpha(buffer, &rect, 255);
    }
    let hover = selected
        || draw.itemState & ODS_SELECTED != 0
        || (!is_menu && draw.itemState & ODS_FOCUS != 0);
    if hover && !disabled {
        let color = if contrast {
            GetSysColor(COLOR_HIGHLIGHT)
        } else {
            0x00ffffff
        };
        let brush = CreateSolidBrush(color);
        let old = SelectObject(dc, brush);
        let pen = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(
            dc,
            rect.left + 2,
            rect.top + 2,
            rect.right - 2,
            rect.bottom - 2,
            8,
            8,
        );
        SelectObject(dc, pen);
        SelectObject(dc, old);
        DeleteObject(brush);
        if contrast {
            BufferedPaintSetAlpha(buffer, &rect, 255);
        } else {
            // Premultiply the rounded mask so the white highlight remains translucent.
            // Setting only alpha on GDI's full-white RGB would produce an opaque halo.
            let mut bits = null_mut();
            let mut stride = 0;
            if GetBufferedPaintBits(buffer, &mut bits, &mut stride) >= 0 && !bits.is_null() {
                let alpha = if dark() { 26 } else { 120 };
                for y in 0..(rect.bottom - rect.top) {
                    for x in 0..(rect.right - rect.left) {
                        let pixel = &mut *bits.offset((y * stride + x) as isize);
                        if pixel.rgbRed != 0 || pixel.rgbGreen != 0 || pixel.rgbBlue != 0 {
                            *pixel = RGBQUAD {
                                rgbRed: alpha,
                                rgbGreen: alpha,
                                rgbBlue: alpha,
                                rgbReserved: alpha,
                            };
                        }
                    }
                }
            }
        }
    }
    let old = SelectObject(dc, font);
    if text == "windows" {
        let color = if contrast {
            GetSysColor(if hover {
                COLOR_HIGHLIGHTTEXT
            } else {
                COLOR_WINDOWTEXT
            })
        } else if dark() {
            0x00f5f5f5
        } else {
            0x00191919
        };
        let brush = CreateSolidBrush(color);
        let cx = (rect.left + rect.right) / 2;
        let cy = (rect.top + rect.bottom) / 2;
        let unit = ((rect.bottom - rect.top) * 4 / 28).max(3);
        for (x, y) in [
            (-unit - 1, -unit - 1),
            (1, -unit - 1),
            (-unit - 1, 1),
            (1, 1),
        ] {
            let square = RECT {
                left: cx + x,
                top: cy + y,
                right: cx + x + unit,
                bottom: cy + y + unit,
            };
            FillRect(dc, &square, brush);
            BufferedPaintSetAlpha(buffer, &square, 255);
        }
        DeleteObject(brush);
        SelectObject(dc, old);
        EndBufferedPaint(buffer, 1);
        return;
    }
    let theme = OpenThemeData(draw.hwndItem, wide("WINDOW").as_ptr());
    let mut options: DTTOPTS = zeroed();
    options.dwSize = size_of::<DTTOPTS>() as u32;
    options.dwFlags = DTT_COMPOSITED | DTT_TEXTCOLOR;
    options.crText = if contrast {
        GetSysColor(if disabled {
            COLOR_GRAYTEXT
        } else if hover {
            COLOR_HIGHLIGHTTEXT
        } else {
            COLOR_WINDOWTEXT
        })
    } else if disabled {
        if dark() { 0x00999999 } else { 0x00666666 }
    } else if dark() {
        0x00f5f5f5
    } else {
        0x00191919
    };
    let mut area = rect;
    if is_menu {
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(draw.hwndItem).max(96) as i32;
        area.left += 34 * dpi / 96;
        area.right -= 22 * dpi / 96;
    }
    let (title, shortcut) = if is_menu {
        text.split_once('\t').unwrap_or((text, ""))
    } else {
        (text, "")
    };
    let value = wide(title);
    DrawThemeTextEx(
        theme,
        dc,
        0,
        0,
        value.as_ptr(),
        -1,
        DT_SINGLELINE
            | DT_VCENTER
            | DT_NOPREFIX
            | DT_END_ELLIPSIS
            | if is_menu || draw.CtlType == ODT_STATIC {
                DT_LEFT
            } else {
                DT_CENTER
            },
        &mut area,
        &options,
    );
    if !shortcut.is_empty() {
        let value = wide(shortcut);
        DrawThemeTextEx(
            theme,
            dc,
            0,
            0,
            value.as_ptr(),
            -1,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_RIGHT,
            &mut area,
            &options,
        );
    }
    if let Some(item) = menu {
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(draw.hwndItem).max(96);
        let icon_font = font_for_menu_icon(dpi);
        SelectObject(dc, icon_font);
        let glyph = if item.checked {
            Some("\u{E73E}")
        } else {
            menu_icon(item)
        };
        if let Some(glyph) = glyph {
            let mut icon_area = RECT {
                left: rect.left + 7 * dpi as i32 / 96,
                right: rect.left + 27 * dpi as i32 / 96,
                ..rect
            };
            let value = wide(glyph);
            DrawThemeTextEx(
                theme,
                dc,
                0,
                0,
                value.as_ptr(),
                -1,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
                &mut icon_area,
                &options,
            );
        }
        if !item.children.is_empty() {
            let mut chevron = RECT {
                left: rect.right - 22 * dpi as i32 / 96,
                right: rect.right - 6 * dpi as i32 / 96,
                ..rect
            };
            let value = wide("\u{E76C}");
            DrawThemeTextEx(
                theme,
                dc,
                0,
                0,
                value.as_ptr(),
                -1,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
                &mut chevron,
                &options,
            );
        }
        SelectObject(dc, font);
        DeleteObject(icon_font);
    }
    CloseThemeData(theme);
    SelectObject(dc, old);
    EndBufferedPaint(buffer, 1);
}

unsafe fn font_for_menu_icon(dpi: u32) -> HFONT {
    sized_font(dpi, 14, false, true)
}

/// Paint over the native trackbar class without replacing its input/UIA provider.
pub unsafe fn paint_slider(hwnd: HWND) {
    let mut paint = zeroed();
    let dc = BeginPaint(hwnd, &mut paint);
    let mut rect = zeroed();
    GetClientRect(hwnd, &mut rect);
    let draw = NMCUSTOMDRAW {
        hdc: dc,
        rc: rect,
        ..zeroed()
    };
    draw_slider(
        &draw,
        SendMessageW(hwnd, WM_USER, 0, 0) as i32,
        GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_DISABLED as isize == 0,
    );
    EndPaint(hwnd, &paint);
}

unsafe fn draw_slider(draw: &NMCUSTOMDRAW, level: i32, enabled: bool) {
    let mut dc = null_mut();
    let rect = draw.rc;
    let params = BP_PAINTPARAMS {
        cbSize: size_of::<BP_PAINTPARAMS>() as u32,
        dwFlags: BPPF_ERASE,
        prcExclude: null(),
        pBlendFunction: null(),
    };
    let buffer = BeginBufferedPaint(draw.hdc, &rect, BPBF_TOPDOWNDIB, &params, &mut dc);
    if buffer == 0 {
        return;
    }
    BufferedPaintClear(buffer, &rect);
    let scale = ((rect.bottom - rect.top) / 30).max(1);
    let y = (rect.top + rect.bottom) / 2;
    let left = rect.left + 9 * scale;
    let right = rect.right - 9 * scale;
    let x = left + (right - left) * level.clamp(0, 100) / 100;
    let pen = SelectObject(dc, GetStockObject(NULL_PEN));
    let remaining = CreateSolidBrush(0x00383838);
    let active = CreateSolidBrush(if enabled { 0x00ffffff } else { 0x00606060 });
    let old = SelectObject(dc, remaining);
    RoundRect(
        dc,
        left,
        y - 2 * scale,
        right,
        y + 2 * scale,
        4 * scale,
        4 * scale,
    );
    SelectObject(dc, active);
    RoundRect(
        dc,
        left,
        y - 2 * scale,
        x,
        y + 2 * scale,
        4 * scale,
        4 * scale,
    );
    Ellipse(
        dc,
        x - 7 * scale,
        y - 7 * scale,
        x + 7 * scale,
        y + 7 * scale,
    );
    SelectObject(dc, old);
    SelectObject(dc, pen);
    DeleteObject(remaining);
    DeleteObject(active);
    let mut bits = null_mut();
    let mut stride = 0;
    if GetBufferedPaintBits(buffer, &mut bits, &mut stride) >= 0 && !bits.is_null() {
        for y in 0..rect.bottom - rect.top {
            for x in 0..rect.right - rect.left {
                let pixel = &mut *bits.offset((y * stride + x) as isize);
                pixel.rgbReserved = pixel.rgbRed;
            }
        }
    }
    EndBufferedPaint(buffer, 1);
}

fn menu_icon(item: &crate::model::Item) -> Option<&'static str> {
    use crate::model::Action;
    match &item.action {
        Action::Uri("ms-settings:about") => Some("\u{E946}"),
        Action::Uri("ms-settings:") => Some("\u{E713}"),
        Action::Uri("ms-windows-store:") => Some("\u{E719}"),
        Action::Uri(uri) if uri.contains("wifi") || uri.contains("network") => Some("\u{E701}"),
        Action::Uri(uri) if uri.contains("bluetooth") => Some("\u{E702}"),
        Action::Uri(uri) if uri.contains("sound") => Some("\u{E767}"),
        Action::Uri(uri) if uri.contains("display") => Some("\u{E7F4}"),
        Action::Lock => Some("\u{E72E}"),
        Action::Exit => Some("\u{E8BB}"),
        Action::Volume(_) | Action::VolumeSet(_) | Action::Mute => Some("\u{E767}"),
        _ if item.label == "Menu Bar Settings" => Some("\u{E713}"),
        _ => None,
    }
}
