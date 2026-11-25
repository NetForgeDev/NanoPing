use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, InvalidateRect,
    RoundRect, SelectObject, SetBkColor, SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
    HBRUSH, HDC, HFONT, OUT_DEFAULT_PRECIS, PS_SOLID, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetWindowLongPtrW, GetWindowRect, LoadCursorW, MessageBoxW,
    PostMessageW, RegisterClassExW, SendMessageW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    ShowWindow, TranslateMessage, BN_CLICKED, BS_OWNERDRAW, GWLP_USERDATA, IDC_ARROW, IDYES,
    MB_ICONQUESTION, MB_YESNO, MSG, SWP_NOZORDER, SW_SHOW, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORDLG, WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_ERASEBKGND, WM_SETFONT, WNDCLASSEXW,
    WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::ui::win_util::{
    apply_exe_icon, enable_dark_title_bar, get_dpi_scale, get_monitor_work_area,
    open_url_in_browser, rgb, scale_px, to_wide, APP_NAME, APP_VERSION, BG_BOX, GITHUB_URL,
};
use crate::updater::{
    check_for_updates, download_and_install_update, UpdateInfo, UpdateStatus,
};

const SS_CENTER: u32 = 0x0001;
const SS_CENTERIMAGE: u32 = 0x0200;
const ODS_SELECTED: u32 = 0x0001;

const WM_APP_ABOUT_EVENT: u32 = WM_APP + 2;

const ID_BTN_UPDATE: u16 = 301;
const ID_BTN_GITHUB: u16 = 302;
const ID_BTN_CLOSE: u16 = 303;

enum AboutEvent {
    CheckDone(UpdateStatus),
    InstallDone(Result<(), String>),
}

struct AboutState {
    hwnd: HWND,
    parent: HWND,
    scale: f32,
    closed: bool,
    checking: bool,
    installing: bool,
    available: Option<UpdateInfo>,
    status_color: u32,
    tx: Sender<AboutEvent>,
    rx: Receiver<AboutEvent>,
    font_bold: HFONT,
    brush_bg: HBRUSH,
    lbl_title: HWND,
    lbl_version: HWND,
    lbl_desc: HWND,
    lbl_credit: HWND,
    lbl_status: HWND,
    btn_update: HWND,
}

impl AboutState {
    unsafe fn set_status(&mut self, text: &str, color: u32) {
        self.status_color = color;
        let w = to_wide(text);
        SetWindowTextW(self.lbl_status, w.as_ptr());
        InvalidateRect(self.lbl_status, ptr::null(), 1);
        InvalidateRect(self.btn_update, ptr::null(), 0);
    }

    unsafe fn start_update_check(&mut self) {
        if self.checking || self.installing {
            return;
        }
        self.checking = true;
        self.set_status("Checking GitHub for updates...", rgb(0xaa, 0xaa, 0xaa));

        let tx = self.tx.clone();
        let hwnd_raw = self.hwnd as isize;
        thread::spawn(move || {
            let status = check_for_updates();
            if tx.send(AboutEvent::CheckDone(status)).is_ok() {
                let _ = PostMessageW(hwnd_raw as HWND, WM_APP_ABOUT_EVENT, 0, 0);
            }
        });
    }

    unsafe fn start_install(&mut self) {
        if self.checking || self.installing {
            return;
        }
        let Some(info) = self.available.clone() else {
            return;
        };

        self.installing = true;
        let display_tag = format_tag(&info.tag);
        self.set_status(
            &format!("Downloading & installing {display_tag}..."),
            rgb(0x00, 0xcc, 0xff),
        );

        let tx = self.tx.clone();
        let hwnd_raw = self.hwnd as isize;
        thread::spawn(move || {
            let res = download_and_install_update(&info.download_url);
            if tx.send(AboutEvent::InstallDone(res)).is_ok() {
                let _ = PostMessageW(hwnd_raw as HWND, WM_APP_ABOUT_EVENT, 0, 0);
            }
        });
    }

    unsafe fn confirm_and_install(&mut self) {
        let Some(ref info) = self.available else {
            return;
        };
        let display_tag = format_tag(&info.tag);
        let prompt = to_wide(&format!(
            "A new version of {APP_NAME} ({display_tag}) is available.\n\
             Current version: v{APP_VERSION}\n\n\
             Do you want to download, replace the current executable, and restart {APP_NAME} now?\n\
             (Your settings in config.ini will be preserved.)"
        ));
        let caption = to_wide(&format!("Update Available — {APP_NAME}"));
        let choice = MessageBoxW(
            self.hwnd,
            prompt.as_ptr(),
            caption.as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        );
        if choice == IDYES {
            self.start_install();
        }
    }

    unsafe fn apply_check_result(&mut self, status: UpdateStatus) {
        self.checking = false;
        match status {
            UpdateStatus::UpToDate => {
                self.available = None;
                self.set_status(&format!("Up to date (v{APP_VERSION})"), rgb(0x00, 0xff, 0x66));
            }
            UpdateStatus::Available(info) => {
                let display_tag = format_tag(&info.tag);
                self.available = Some(info);
                self.set_status(
                    &format!("New release available: {display_tag}"),
                    rgb(0x00, 0xcc, 0xff),
                );
                self.confirm_and_install();
            }
            UpdateStatus::NoReleases => {
                self.available = None;
                self.set_status("No releases published on GitHub yet", rgb(0xff, 0xaa, 0x00));
            }
            UpdateStatus::Failed => {
                self.available = None;
                self.set_status("Could not connect to GitHub", rgb(0xff, 0x44, 0x44));
            }
        }
    }

    unsafe fn apply_install_result(&mut self, res: Result<(), String>) {
        self.installing = false;
        match res {
            Ok(()) => {
                // New NanoPing.exe is already starting and will remove `.old` automatically.
                std::process::exit(0);
            }
            Err(err) => {
                self.set_status(&format!("Update failed: {err}"), rgb(0xff, 0x44, 0x44));
            }
        }
    }

    unsafe fn draw_button(&self, dis: &DRAWITEMSTRUCT) {
        let id = dis.CtlID as u16;
        let pressed = (dis.itemState & ODS_SELECTED) != 0;

        let (text, bg, fg): (String, u32, u32) = match id {
            ID_BTN_UPDATE => {
                if self.installing {
                    (
                        "UPDATING...".to_string(),
                        rgb(0x2a, 0x2a, 0x2a),
                        rgb(0x00, 0xcc, 0xff),
                    )
                } else if self.checking {
                    (
                        "CHECKING...".to_string(),
                        rgb(0x2a, 0x2a, 0x2a),
                        rgb(0x88, 0x88, 0x88),
                    )
                } else if let Some(ref info) = self.available {
                    let display_tag = format_tag(&info.tag);
                    (
                        format!("INSTALL {display_tag}"),
                        rgb(0x00, 0x99, 0x44),
                        rgb(0xff, 0xff, 0xff),
                    )
                } else {
                    (
                        "CHECK FOR UPDATES".to_string(),
                        rgb(0x1e, 0x2a, 0x38),
                        rgb(0x00, 0xcc, 0xff),
                    )
                }
            }
            ID_BTN_GITHUB => (
                "GITHUB".to_string(),
                rgb(0x22, 0x22, 0x22),
                rgb(0xff, 0xff, 0xff),
            ),
            ID_BTN_CLOSE => (
                "CLOSE".to_string(),
                rgb(0x33, 0x33, 0x33),
                rgb(0xcc, 0xcc, 0xcc),
            ),
            _ => (String::new(), rgb(0x22, 0x22, 0x22), rgb(0xff, 0xff, 0xff)),
        };

        let fill_color = if pressed {
            rgb(
                ((bg & 0xFF) as u16 * 3 / 4) as u8,
                (((bg >> 8) & 0xFF) as u16 * 3 / 4) as u8,
                (((bg >> 16) & 0xFF) as u16 * 3 / 4) as u8,
            )
        } else {
            bg
        };

        let hdc = dis.hDC;
        FillRect(hdc, &dis.rcItem, self.brush_bg);

        let brush = CreateSolidBrush(fill_color);
        let pen = CreatePen(PS_SOLID as i32, 1, rgb(0x2e, 0x2e, 0x2e));
        let old_b = SelectObject(hdc, brush as _);
        let old_p = SelectObject(hdc, pen as _);
        let corner = scale_px(4, self.scale);
        RoundRect(
            hdc,
            dis.rcItem.left,
            dis.rcItem.top,
            dis.rcItem.right,
            dis.rcItem.bottom,
            corner,
            corner,
        );
        SelectObject(hdc, old_b);
        SelectObject(hdc, old_p);
        DeleteObject(brush as _);
        DeleteObject(pen as _);

        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, fg);
        SelectObject(hdc, self.font_bold as _);
        let wide = to_wide(&text);
        let mut r = dis.rcItem;
        DrawTextW(
            hdc,
            wide.as_ptr(),
            -1,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }

    unsafe fn close(&mut self) {
        if self.installing {
            return;
        }
        self.closed = true;
        EnableWindow(self.parent, 1);
        SetFocus(self.parent);
        DestroyWindow(self.hwnd);
    }
}

fn format_tag(tag: &str) -> String {
    if tag.starts_with('v') || tag.starts_with('V') {
        tag.to_string()
    } else {
        format!("v{tag}")
    }
}

unsafe extern "system" fn about_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AboutState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = &mut *state_ptr;

    match msg {
        WM_ERASEBKGND => {
            let hdc = wparam as HDC;
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            FillRect(hdc, &rc, state.brush_bg);
            1
        }
        WM_CTLCOLORDLG | WM_CTLCOLORSTATIC => {
            let hdc = wparam as HDC;
            let ctrl = lparam as HWND;
            let color = if ctrl == state.lbl_title {
                rgb(0xff, 0xff, 0xff)
            } else if ctrl == state.lbl_version {
                rgb(0x00, 0xff, 0x66)
            } else if ctrl == state.lbl_desc {
                rgb(0xbb, 0xbb, 0xbb)
            } else if ctrl == state.lbl_credit {
                rgb(0x77, 0x77, 0x77)
            } else if ctrl == state.lbl_status {
                state.status_color
            } else {
                rgb(0xbb, 0xbb, 0xbb)
            };
            SetTextColor(hdc, color);
            SetBkColor(hdc, BG_BOX);
            state.brush_bg as LRESULT
        }
        WM_DRAWITEM => {
            let dis = &*(lparam as *const DRAWITEMSTRUCT);
            state.draw_button(dis);
            1
        }
        WM_APP_ABOUT_EVENT => {
            while let Ok(ev) = state.rx.try_recv() {
                match ev {
                    AboutEvent::CheckDone(s) => state.apply_check_result(s),
                    AboutEvent::InstallDone(r) => state.apply_install_result(r),
                }
            }
            0
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as u16;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            if code == BN_CLICKED {
                match id {
                    ID_BTN_UPDATE => {
                        if state.available.is_some() {
                            state.confirm_and_install();
                        } else {
                            state.start_update_check();
                        }
                    }
                    ID_BTN_GITHUB => open_url_in_browser(GITHUB_URL),
                    ID_BTN_CLOSE => state.close(),
                    _ => {}
                }
            }
            0
        }
        WM_CLOSE => {
            state.close();
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn show_about_dialog(parent: HWND) {
    unsafe {
        let hinst = GetModuleHandleW(ptr::null());
        let class_name = to_wide("NanoPingAboutWindow");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(about_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        RegisterClassExW(&wc);

        let title = to_wide(&format!("About {APP_NAME}"));
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            0,
            0,
            300,
            240,
            parent,
            ptr::null_mut(),
            hinst,
            ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }

        enable_dark_title_bar(hwnd);
        apply_exe_icon(hwnd);

        let scale = get_dpi_scale(hwnd);
        let px = |v: i32| scale_px(v, scale);
        let segoe = to_wide("Segoe UI");

        let mk_font = |h: i32, weight: i32| -> HFONT {
            CreateFontW(
                -px(h),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                DEFAULT_PITCH as u32,
                segoe.as_ptr(),
            )
        };

        let font_title = mk_font(20, 700);
        let font_regular = mk_font(13, 400);
        let font_bold = mk_font(12, 700);
        let font_small = mk_font(11, 400);
        let brush_bg = CreateSolidBrush(BG_BOX);

        let pad = px(16);
        let client_w = px(290);
        let content_w = client_w - pad * 2;
        let cls_static = to_wide("STATIC");
        let cls_btn = to_wide("BUTTON");

        let make_ctrl = |class: &[u16],
                         text: &str,
                         style: u32,
                         x: i32,
                         y: i32,
                         w: i32,
                         h: i32,
                         id: u16,
                         font: HFONT|
         -> HWND {
            let wt = to_wide(text);
            let ch = CreateWindowExW(
                0,
                class.as_ptr(),
                wt.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                x,
                y,
                w,
                h,
                hwnd,
                id as usize as _,
                hinst,
                ptr::null(),
            );
            SendMessageW(ch, WM_SETFONT, font as usize, 1);
            ch
        };

        let mut y = px(14);
        let lbl_title = make_ctrl(
            &cls_static,
            APP_NAME,
            SS_CENTER | SS_CENTERIMAGE,
            pad,
            y,
            content_w,
            px(26),
            0,
            font_title,
        );
        y += px(26);

        let lbl_version = make_ctrl(
            &cls_static,
            &format!("Version {APP_VERSION}"),
            SS_CENTER | SS_CENTERIMAGE,
            pad,
            y,
            content_w,
            px(18),
            0,
            font_bold,
        );
        y += px(22);

        let lbl_desc = make_ctrl(
            &cls_static,
            "Ultra-lightweight native Windows ping monitor",
            SS_CENTER | SS_CENTERIMAGE,
            pad,
            y,
            content_w,
            px(18),
            0,
            font_regular,
        );
        y += px(18);

        let lbl_credit = make_ctrl(
            &cls_static,
            "NetForgeDev  •  MIT License",
            SS_CENTER | SS_CENTERIMAGE,
            pad,
            y,
            content_w,
            px(18),
            0,
            font_small,
        );
        y += px(26);

        let lbl_status = make_ctrl(
            &cls_static,
            "Ready to check for updates",
            SS_CENTER | SS_CENTERIMAGE,
            pad,
            y,
            content_w,
            px(20),
            0,
            font_small,
        );
        y += px(24);

        let btn_update = make_ctrl(
            &cls_btn,
            "CHECK FOR UPDATES",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            pad,
            y,
            content_w,
            px(26),
            ID_BTN_UPDATE,
            font_bold,
        );
        y += px(34);

        let b_gap = px(8);
        let half_w = (content_w - b_gap) / 2;
        make_ctrl(
            &cls_btn,
            "GITHUB",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            pad,
            y,
            half_w,
            px(26),
            ID_BTN_GITHUB,
            font_bold,
        );
        make_ctrl(
            &cls_btn,
            "CLOSE",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            pad + half_w + b_gap,
            y,
            half_w,
            px(26),
            ID_BTN_CLOSE,
            font_bold,
        );
        y += px(26) + px(14);

        let mut r = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: y,
        };
        AdjustWindowRectEx(
            &mut r,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            0,
            WS_EX_DLGMODALFRAME,
        );
        let win_w = r.right - r.left;
        let win_h = r.bottom - r.top;

        let mut pr: RECT = std::mem::zeroed();
        GetWindowRect(parent, &mut pr);
        let mon = get_monitor_work_area(parent);
        let mut x = pr.left + ((pr.right - pr.left) - win_w) / 2;
        let mut top_y = pr.top + ((pr.bottom - pr.top) - win_h) / 2;
        if x < mon.left + 10 {
            x = mon.left + 10;
        } else if x + win_w > mon.right - 10 {
            x = mon.right - win_w - 10;
        }
        if top_y < mon.top + 10 {
            top_y = mon.top + 10;
        } else if top_y + win_h > mon.bottom - 10 {
            top_y = mon.bottom - win_h - 10;
        }
        SetWindowPos(hwnd, ptr::null_mut(), x, top_y, win_w, win_h, SWP_NOZORDER);

        let (tx, rx) = mpsc::channel();
        let mut state = Box::new(AboutState {
            hwnd,
            parent,
            scale,
            closed: false,
            checking: false,
            installing: false,
            available: None,
            status_color: rgb(0x77, 0x77, 0x77),
            tx,
            rx,
            font_bold,
            brush_bg,
            lbl_title,
            lbl_version,
            lbl_desc,
            lbl_credit,
            lbl_status,
            btn_update,
        });

        let state_ptr = &mut *state as *mut AboutState;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

        EnableWindow(parent, 0);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while !state.closed && GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        DeleteObject(font_title as _);
        DeleteObject(font_regular as _);
        DeleteObject(font_bold as _);
        DeleteObject(font_small as _);
        DeleteObject(brush_bg as _);
    }
}