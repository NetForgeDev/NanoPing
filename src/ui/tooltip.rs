use std::ptr;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, GetDC, GetTextExtentPoint32W, InvalidateRect, Rectangle, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HFONT, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    RegisterClassExW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA, HWND_TOPMOST,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::ui::win_util::{get_monitor_work_area, rgb, scale_px, to_wide};

struct TooltipData {
    text: String,
    font: HFONT,
}

pub struct DarkTooltip {
    hwnd: HWND,
    font: HFONT,
    scale: f32,
}

unsafe extern "system" fn tooltip_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TooltipData;
    if data_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let data = &*data_ptr;
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);

            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);

            let bg_brush = CreateSolidBrush(rgb(0x16, 0x16, 0x16));
            let border_pen = CreatePen(PS_SOLID as i32, 1, rgb(0x3a, 0x3a, 0x3a));
            let old_b = SelectObject(hdc, bg_brush as _);
            let old_p = SelectObject(hdc, border_pen as _);
            Rectangle(hdc, rc.left, rc.top, rc.right, rc.bottom);
            SelectObject(hdc, old_b);
            SelectObject(hdc, old_p);
            DeleteObject(bg_brush as _);
            DeleteObject(border_pen as _);

            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, rgb(0xe8, 0xe8, 0xe8));
            SelectObject(hdc, data.font as _);
            let wide = to_wide(&data.text);
            DrawTextW(
                hdc,
                wide.as_ptr(),
                -1,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );

            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let _ = Box::from_raw(data_ptr);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl DarkTooltip {
    pub unsafe fn new(owner: HWND, scale: f32) -> Self {
        let hinst = GetModuleHandleW(ptr::null());
        let class_name = to_wide("NanoPingDarkTooltip");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(tooltip_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        RegisterClassExW(&wc);

        let segoe = to_wide("Segoe UI");
        let font = CreateFontW(
            -scale_px(12, scale),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            segoe.as_ptr(),
        );

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class_name.as_ptr(),
            ptr::null(),
            WS_POPUP,
            0,
            0,
            10,
            10,
            owner,
            ptr::null_mut(),
            hinst,
            ptr::null(),
        );

        if !hwnd.is_null() {
            let data = Box::new(TooltipData {
                text: String::new(),
                font,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
        }

        Self { hwnd, font, scale }
    }

    pub unsafe fn show(&self, owner: HWND, anchor_client: RECT, text: &str) {
        if self.hwnd.is_null() {
            return;
        }

        let data_ptr = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *mut TooltipData;
        if data_ptr.is_null() {
            return;
        }
        (*data_ptr).text = text.to_string();

        // Measure text size
        let hdc = GetDC(self.hwnd);
        let old = SelectObject(hdc, self.font as _);
        let wide = to_wide(text);
        let mut sz = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(hdc, wide.as_ptr(), (wide.len() - 1) as i32, &mut sz);
        SelectObject(hdc, old);
        ReleaseDC(self.hwnd, hdc);

        let tip_w = sz.cx + scale_px(14, self.scale);
        let tip_h = sz.cy + scale_px(8, self.scale);

        // Convert button rect to screen coordinates
        let mut tl = POINT {
            x: anchor_client.left,
            y: anchor_client.top,
        };
        let mut br = POINT {
            x: anchor_client.right,
            y: anchor_client.bottom,
        };
        ClientToScreen(owner, &mut tl);
        ClientToScreen(owner, &mut br);

        let mon = get_monitor_work_area(owner);
        let gap = scale_px(5, self.scale);

        let mut x = (tl.x + br.x) / 2 - tip_w / 2;
        let mut y = br.y + gap;

        if x < mon.left + 4 {
            x = mon.left + 4;
        } else if x + tip_w > mon.right - 4 {
            x = mon.right - tip_w - 4;
        }
        if y + tip_h > mon.bottom - 4 {
            y = tl.y - tip_h - gap;
        }

        SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, tip_w, tip_h, SWP_NOACTIVATE);
        ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        InvalidateRect(self.hwnd, ptr::null(), 0);
    }

    pub unsafe fn hide(&self) {
        if !self.hwnd.is_null() {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for DarkTooltip {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            if !self.font.is_null() {
                DeleteObject(self.font as _);
            }
        }
    }
}