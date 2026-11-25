use std::ffi::c_void;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    GetDC, GetDeviceCaps, GetMonitorInfoW, MonitorFromRect, MonitorFromWindow, ReleaseDC,
    LOGPIXELSX, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconW, SetCurrentProcessExplicitAppUserModelID, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SendMessageW, SetProcessDPIAware, ICON_BIG, ICON_SMALL, SM_CXSCREEN,
    SM_CYSCREEN, SW_SHOWNORMAL, WM_SETICON,
};
use windows_sys::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};

use crate::config::{PingMethod, WaveSpeed};

pub const BG_MAIN: u32 = rgb(0x00, 0x00, 0x00);
pub const BG_BOX: u32 = rgb(0x11, 0x11, 0x11);
pub const BG_BTN: u32 = rgb(0x18, 0x18, 0x18);
pub const BG_BTN_HOVER: u32 = rgb(0x28, 0x28, 0x28);

pub const APP_NAME: &str = "NanoPing";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_REPO: &str = "NetForgeDev/NanoPing";
pub const GITHUB_URL: &str = "https://github.com/NetForgeDev/NanoPing";

/// Builds a Win32 GDI `COLORREF` (`0x00BBGGRR`).
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

pub fn lerp_color(c1: u32, c2: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let r1 = (c1 & 0xFF) as f32;
    let g1 = ((c1 >> 8) & 0xFF) as f32;
    let b1 = ((c1 >> 16) & 0xFF) as f32;

    let r2 = (c2 & 0xFF) as f32;
    let g2 = ((c2 >> 8) & 0xFF) as f32;
    let b2 = ((c2 >> 16) & 0xFF) as f32;

    let r = (r1 + (r2 - r1) * t) as u8;
    let g = (g1 + (g2 - g1) * t) as u8;
    let b = (b1 + (b2 - b1) * t) as u8;
    rgb(r, g, b)
}

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn init_process() {
    unsafe {
        let _ = SetProcessDPIAware();
        let app_id = to_wide("NetForgeDev.NanoPing.Live");
        let _ = SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());
    }
}

pub fn enable_dark_title_bar(hwnd: HWND) {
    unsafe {
        let val: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            20, // DWMWA_USE_IMMERSIVE_DARK_MODE (Windows 11 / Win10 20H1+)
            &val as *const i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            19, // Fallback for older Windows 10 builds
            &val as *const i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

pub fn apply_exe_icon(hwnd: HWND) {
    if let Ok(exe) = std::env::current_exe() {
        let wide = to_wide(&exe.to_string_lossy());
        unsafe {
            let h_icon = ExtractIconW(ptr::null_mut(), wide.as_ptr(), 0);
            if (h_icon as usize) > 1 {
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, h_icon as isize);
                SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, h_icon as isize);
            }
        }
    }
}

pub fn open_file_in_shell(path: &Path) {
    let verb = to_wide("open");
    let file = to_wide(&path.to_string_lossy());
    unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

pub fn open_url_in_browser(url: &str) {
    let verb = to_wide("open");
    let target = to_wide(url);
    unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

pub fn get_dpi_scale(hwnd: HWND) -> f32 {
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return 1.0;
        }
        let dpi = GetDeviceCaps(hdc, LOGPIXELSX as i32);
        ReleaseDC(hwnd, hdc);
        if dpi > 0 {
            dpi as f32 / 96.0
        } else {
            1.0
        }
    }
}

pub fn scale_px(px: i32, scale: f32) -> i32 {
    ((px as f32) * scale).round() as i32
}

pub fn get_monitor_work_area(hwnd: HWND) -> RECT {
    unsafe {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            return mi.rcWork;
        }
        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        }
    }
}

pub fn method_color(method: PingMethod) -> u32 {
    match method {
        PingMethod::Icmp => rgb(0xff, 0x99, 0x00),
        PingMethod::Tcp => rgb(0x00, 0xcc, 0xff),
        PingMethod::Udp => rgb(0xff, 0x66, 0xcc),
    }
}

pub fn speed_color(speed: WaveSpeed) -> u32 {
    match speed {
        WaveSpeed::Normal => rgb(0x00, 0xcc, 0xff),
        WaveSpeed::Slow => rgb(0xff, 0xaa, 0x00),
        WaveSpeed::Fast => rgb(0x00, 0xff, 0x66),
        WaveSpeed::VeryFast => rgb(0xff, 0x33, 0xcc),
    }
}
/// Checks whether a saved position is visible on any currently connected monitor.
/// If that monitor was disconnected, centers on the primary monitor's work area.
/// If partially off-screen, clamps safely inside the monitor's work area.
pub fn resolve_safe_window_position(
    saved_x: Option<i32>,
    saved_y: Option<i32>,
    win_w: i32,
    win_h: i32,
) -> (i32, i32, bool) {
    unsafe {
        if let (Some(sx), Some(sy)) = (saved_x, saved_y) {
            let rect = RECT {
                left: sx,
                top: sy,
                right: sx + win_w,
                bottom: sy + win_h,
            };
            let hmon = MonitorFromRect(&rect, MONITOR_DEFAULTTONULL);
            if !hmon.is_null() {
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(hmon, &mut mi) != 0 {
                    let w = mi.rcWork;
                    let max_x = (w.right - win_w).max(w.left);
                    let max_y = (w.bottom - win_h).max(w.top);
                    let cx = sx.clamp(w.left, max_x);
                    let cy = sy.clamp(w.top, max_y);
                    return (cx, cy, cx != sx || cy != sy);
                }
            }
        }

        // Disconnected monitor or 'center': center on primary monitor work area
        let origin = RECT {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        };
        let hmon = MonitorFromRect(&origin, MONITOR_DEFAULTTOPRIMARY);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !hmon.is_null() && GetMonitorInfoW(hmon, &mut mi) != 0 {
            let w = mi.rcWork;
            let cx = w.left + ((w.right - w.left) - win_w) / 2;
            let cy = w.top + ((w.bottom - w.top) - win_h) / 2;
            return (cx, cy, true);
        }

        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        ((sw - win_w) / 2, (sh - win_h) / 2, true)
    }
}

pub fn parse_hex_color(s: &str) -> Option<u32> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(rgb(r, g, b))
}

pub fn format_hex_color(c: u32) -> String {
    let r = c & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = (c >> 16) & 0xFF;
    format!("#{r:02x}{g:02x}{b:02x}")
}

pub fn pick_color_dialog(owner: HWND, initial: u32) -> Option<u32> {
    unsafe {
        let mut custom: [u32; 16] = [
            rgb(0x00, 0xcc, 0x55),
            rgb(0x00, 0xff, 0x66),
            rgb(0x00, 0xcc, 0xff),
            rgb(0x33, 0x99, 0xff),
            rgb(0xe6, 0xe6, 0x00),
            rgb(0xff, 0xcc, 0x00),
            rgb(0xe6, 0x8a, 0x00),
            rgb(0xff, 0x55, 0x00),
            rgb(0xcc, 0x33, 0x33),
            rgb(0xff, 0x22, 0x22),
            rgb(0xff, 0x66, 0xcc),
            rgb(0xbb, 0x66, 0xff),
            rgb(0xff, 0xff, 0xff),
            rgb(0xbb, 0xbb, 0xbb),
            rgb(0x77, 0x77, 0x77),
            rgb(0x44, 0x44, 0x44),
        ];
        let mut cc: CHOOSECOLORW = std::mem::zeroed();
        cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
        cc.hwndOwner = owner;
        cc.rgbResult = initial;
        cc.lpCustColors = custom.as_mut_ptr();
        cc.Flags = CC_RGBINIT | CC_FULLOPEN;
        if ChooseColorW(&mut cc) != 0 {
            Some(cc.rgbResult)
        } else {
            None
        }
    }
}