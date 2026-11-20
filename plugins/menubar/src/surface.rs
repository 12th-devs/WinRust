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
    let material: i32 = if crate::CONFIG.with_borrow(|c| c.acrylic) {
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
    let border = 0xffff_fffeu32; // DWMWA_COLOR_NONE
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
    CreateFontW(
        -(13 * dpi as i32 / 96),
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
    is_menu: bool,
) {
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
    let disabled = draw.itemState & ODS_DISABLED != 0;
    let contrast = high_contrast();
    if contrast {
        FillRect(dc, &rect, GetSysColorBrush(COLOR_WINDOW));
        BufferedPaintSetAlpha(buffer, &rect, 255);
    }
    let hover = selected || draw.itemState & (ODS_SELECTED | ODS_FOCUS | ODS_HOTLIGHT) != 0;
    if hover && !disabled {
        let color = if contrast {
            GetSysColor(COLOR_HIGHLIGHT)
        } else if dark() {
            0x00505050
        } else {
            0x00dedede
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
        BufferedPaintSetAlpha(
            buffer,
            &RECT {
                left: rect.left + 2,
                top: rect.top + 2,
                right: rect.right - 2,
                bottom: rect.bottom - 2,
            },
            255,
        );
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
        area.left += 13;
        area.right -= 13;
    }
    let value = wide(text);
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
            | if is_menu {
                DT_LEFT | DT_EXPANDTABS
            } else {
                DT_CENTER
            },
        &mut area,
        &options,
    );
    if draw.itemState & ODS_FOCUS != 0 {
        DrawFocusRect(
            dc,
            &RECT {
                left: rect.left + 3,
                top: rect.top + 3,
                right: rect.right - 3,
                bottom: rect.bottom - 3,
            },
        );
    }
    CloseThemeData(theme);
    SelectObject(dc, old);
    EndBufferedPaint(buffer, 1);
}
