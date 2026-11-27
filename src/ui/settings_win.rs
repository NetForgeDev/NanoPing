use std::ptr;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, InvalidateRect,
    RoundRect, SelectObject, SetBkColor, SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HDC, HFONT, OUT_DEFAULT_PRECIS, PS_SOLID, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, LoadCursorW, MessageBoxW, RegisterClassExW, SendMessageW, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, BN_CLICKED, BS_OWNERDRAW,
    GWLP_USERDATA, IDC_ARROW, MB_ICONERROR, MB_OK, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW,
    WM_CLOSE, WM_COMMAND, WM_CTLCOLORDLG, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DRAWITEM,
    WM_ERASEBKGND, WM_SETFONT, WNDCLASSEXW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_DLGMODALFRAME,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::config::{config_path, Config, PingColors, PingMethod, WaveSpeed};
use crate::ui::about_win::show_about_dialog;
use crate::ui::main_win::WM_APP_PREVIEW_CONFIG;
use crate::ui::win_util::{
    apply_exe_icon, enable_dark_title_bar, get_dpi_scale, get_monitor_work_area, method_color,
    open_file_in_shell, pick_color_dialog, rgb, scale_px, speed_color, to_wide, APP_NAME,
    APP_VERSION, BG_BOX,
};

const ES_CENTER: u32 = 0x0001;
const ES_AUTOHSCROLL: u32 = 0x0080;
const EN_CHANGE_CODE: u32 = 0x0300;
const SS_LEFT: u32 = 0x0000;
const SS_CENTERIMAGE: u32 = 0x0200;
const ODS_SELECTED: u32 = 0x0001;

const ID_ADD_TARGET: u16 = 100;
const ID_METHOD_BTN: u16 = 101;
const ID_TOP_BTN: u16 = 102;
const ID_SAVE_POS_BTN: u16 = 103;
const ID_AUTO_BTN: u16 = 104;
const ID_WAVE_BTN: u16 = 105;
const ID_SPEED_BTN: u16 = 106;
const ID_DEFAULTS_BTN: u16 = 107;
const ID_OPEN_INI_BTN: u16 = 108;
const ID_SAVE_BTN: u16 = 109;
const ID_ABOUT_BTN: u16 = 110;
const ID_FONT_DEC_BTN: u16 = 111;
const ID_FONT_INC_BTN: u16 = 112;
const ID_COLOR_LOW_BTN: u16 = 113;
const ID_COLOR_MED_BTN: u16 = 114;
const ID_COLOR_HIGH_BTN: u16 = 115;
const ID_COLOR_TO_BTN: u16 = 116;
const ID_OPACITY_DEC_BTN: u16 = 117;
const ID_OPACITY_INC_BTN: u16 = 118;
const ID_CHECK_UPD_BTN: u16 = 119;

const ID_EDIT_HIST: u16 = 200;
const ID_EDIT_LOSS: u16 = 201;
const ID_EDIT_AVG: u16 = 202;
const ID_EDIT_REF: u16 = 203;
const ID_EDIT_TIMEOUT: u16 = 204;
const ID_EDIT_FONT: u16 = 205;
const ID_EDIT_OPACITY: u16 = 206;

const ID_TARGET_EDIT_BASE: u16 = 1000;
const ID_TARGET_DEL_BASE: u16 = 2000;

struct Draft {
    targets: Vec<String>,
    method: PingMethod,
    hist: String,
    loss: String,
    avg: String,
    refresh: String,
    timeout: String,
    always_on_top: bool,
    save_position: bool,
    autostart: bool,
    check_updates: bool,
    wave_effect: bool,
    wave_speed: WaveSpeed,
    font_size: String,
    opacity: String,
    colors: PingColors,
}

impl Draft {
    fn from_config(c: &Config) -> Self {
        Self {
            targets: c.targets.clone(),
            method: c.ping_method,
            hist: c.history_count.to_string(),
            loss: c.loss_window_size.to_string(),
            avg: c.avg_window_size.to_string(),
            refresh: c.refresh_interval_ms.to_string(),
            timeout: c.timeout_ms.to_string(),
            always_on_top: c.always_on_top,
            save_position: c.save_position,
            autostart: c.autostart,
            check_updates: c.check_updates,
            wave_effect: c.wave_effect,
            wave_speed: c.wave_speed,
            font_size: c.font_size.to_string(),
            opacity: c.opacity.to_string(),
            colors: c.colors,
        }
    }
}

struct SettingsState {
    hwnd: HWND,
    parent: HWND,
    scale: f32,
    draft: Draft,
    base_config: Config,
    result: Option<Config>,
    closed: bool,
    building_ui: bool,
    font_regular: HFONT,
    font_bold: HFONT,
    brush_bg: HBRUSH,
    brush_edit: HBRUSH,
    child_hwnds: Vec<HWND>,
    target_edits: Vec<HWND>,
    edit_hist: HWND,
    edit_loss: HWND,
    edit_avg: HWND,
    edit_ref: HWND,
    edit_timeout: HWND,
    edit_font: HWND,
    edit_opacity: HWND,
}

unsafe fn get_edit_text(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let len = GetWindowTextLengthW(hwnd);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..copied as usize])
}

impl SettingsState {
    unsafe fn sync_edits_to_draft(&mut self) {
        self.draft.targets = self.target_edits.iter().map(|&h| get_edit_text(h)).collect();
        self.draft.hist = get_edit_text(self.edit_hist);
        self.draft.loss = get_edit_text(self.edit_loss);
        self.draft.avg = get_edit_text(self.edit_avg);
        self.draft.refresh = get_edit_text(self.edit_ref);
        self.draft.timeout = get_edit_text(self.edit_timeout);
        self.draft.font_size = get_edit_text(self.edit_font);
        self.draft.opacity = get_edit_text(self.edit_opacity);
    }

    unsafe fn emit_live_preview(&mut self) {
        if self.building_ui {
            return;
        }
        self.sync_edits_to_draft();

        let targets: Vec<String> = self
            .draft
            .targets
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut cfg = self.base_config.clone();
        if !targets.is_empty() {
            cfg.targets = targets;
        }
        cfg.ping_method = self.draft.method;
        if let Ok(v) = self.draft.hist.trim().parse::<usize>() {
            if v >= 1 {
                cfg.history_count = v.clamp(1, 20);
            }
        }
        if let Ok(v) = self.draft.loss.trim().parse::<usize>() {
            if v >= 1 {
                cfg.loss_window_size = v;
            }
        }
        if let Ok(v) = self.draft.avg.trim().parse::<usize>() {
            if v >= 1 {
                cfg.avg_window_size = v;
            }
        }
        if let Ok(v) = self.draft.refresh.trim().parse::<u64>() {
            if v >= 50 {
                cfg.refresh_interval_ms = v;
            }
        }
        if let Ok(v) = self.draft.timeout.trim().parse::<u64>() {
            if v >= 100 {
                cfg.timeout_ms = v;
            }
        }
        if let Ok(v) = self.draft.font_size.trim().parse::<u32>() {
            if (9..=28).contains(&v) {
                cfg.font_size = v;
            }
        }
        if let Ok(v) = self.draft.opacity.trim().parse::<u8>() {
            if (25..=100).contains(&v) {
                cfg.opacity = v;
            }
        }
        cfg.always_on_top = self.draft.always_on_top;
        cfg.save_position = self.draft.save_position;
        cfg.autostart = self.draft.autostart;
        cfg.check_updates = self.draft.check_updates;
        cfg.wave_effect = self.draft.wave_effect;
        cfg.wave_speed = self.draft.wave_speed;
        cfg.colors = self.draft.colors;

        SendMessageW(
            self.parent,
            WM_APP_PREVIEW_CONFIG,
            0,
            &cfg as *const Config as isize,
        );
    }

    unsafe fn step_font_size(&mut self, delta: i32) {
        self.sync_edits_to_draft();
        let cur = self
            .draft
            .font_size
            .trim()
            .parse::<i32>()
            .unwrap_or(self.base_config.font_size as i32);
        let next = (cur + delta).clamp(9, 28) as u32;
        self.draft.font_size = next.to_string();

        self.building_ui = true;
        let w = to_wide(&self.draft.font_size);
        SetWindowTextW(self.edit_font, w.as_ptr());
        self.building_ui = false;

        self.emit_live_preview();
    }

    unsafe fn step_opacity(&mut self, delta: i32) {
        self.sync_edits_to_draft();
        let cur = self
            .draft
            .opacity
            .trim()
            .parse::<i32>()
            .unwrap_or(self.base_config.opacity as i32);
        let next = (cur + delta).clamp(25, 100) as u8;
        self.draft.opacity = next.to_string();

        self.building_ui = true;
        let w = to_wide(&self.draft.opacity);
        SetWindowTextW(self.edit_opacity, w.as_ptr());
        self.building_ui = false;

        self.emit_live_preview();
    }

    unsafe fn rebuild_ui(&mut self, center_on_parent: bool) {
        self.building_ui = true;

        for h in self.child_hwnds.drain(..) {
            DestroyWindow(h);
        }
        self.target_edits.clear();

        let s = self.scale;
        let px = |v: i32| scale_px(v, s);
        let hinst = GetModuleHandleW(ptr::null());

        let pad = px(12);
        let col_w = px(200);
        let col_gap = px(16);
        let client_w = pad * 2 + col_w * 2 + col_gap;
        let content_w = client_w - pad * 2;
        let mut y = px(10);

        let cls_static = to_wide("STATIC");
        let cls_edit = to_wide("EDIT");
        let cls_btn = to_wide("BUTTON");

        let mut add_ctrl = |class: &[u16],
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
                self.hwnd,
                id as usize as _,
                hinst,
                ptr::null(),
            );
            SendMessageW(ch, WM_SETFONT, font as usize, 1);
            self.child_hwnds.push(ch);
            ch
        };

        // Header: Targets + [+ Add] + [ABOUT]
        let top_btn_w = px(58);
        let top_gap = px(5);
        add_ctrl(
            &cls_static,
            "Target Addresses:",
            SS_LEFT | SS_CENTERIMAGE,
            pad,
            y,
            content_w - top_btn_w * 2 - top_gap * 2,
            px(20),
            0,
            self.font_bold,
        );
        add_ctrl(
            &cls_btn,
            "+ Add",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            client_w - pad - top_btn_w * 2 - top_gap,
            y,
            top_btn_w,
            px(20),
            ID_ADD_TARGET,
            self.font_bold,
        );
        add_ctrl(
            &cls_btn,
            "ABOUT",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            client_w - pad - top_btn_w,
            y,
            top_btn_w,
            px(20),
            ID_ABOUT_BTN,
            self.font_bold,
        );
        y += px(25);

        // 3-column Targets grid
        let t_gap = px(6);
        let t_col_w = (content_w - t_gap * 2) / 3;
        let del_w = px(20);
        let ent_w = t_col_w - del_w - px(2);
        let row_h = px(21);

        let targets_snapshot = self.draft.targets.clone();
        for (idx, val) in targets_snapshot.iter().enumerate() {
            let r = (idx / 3) as i32;
            let c = (idx % 3) as i32;
            let rx = pad + c * (t_col_w + t_gap);
            let ry = y + r * (row_h + px(4));

            let eh = add_ctrl(
                &cls_edit,
                val,
                WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL,
                rx,
                ry,
                ent_w,
                row_h,
                ID_TARGET_EDIT_BASE + idx as u16,
                self.font_regular,
            );
            self.target_edits.push(eh);

            add_ctrl(
                &cls_btn,
                "X",
                WS_TABSTOP | BS_OWNERDRAW as u32,
                rx + ent_w + px(2),
                ry,
                del_w,
                row_h,
                ID_TARGET_DEL_BASE + idx as u16,
                self.font_bold,
            );
        }

        let target_rows = ((targets_snapshot.len() + 2) / 3).max(1) as i32;
        y += target_rows * (row_h + px(4)) + px(8);

        // 2-column settings grid
        let left_x = pad;
        let right_col_x = pad + col_w + col_gap;
        let ctrl_w = px(72);
        let lbl_w = col_w - ctrl_w - px(4);
        let item_h = px(21);
        let item_step = px(26);

        // Left column: Ping Method + 5 numeric settings
        let mut ly = y;
        add_ctrl(
            &cls_static,
            "Ping Method:",
            SS_LEFT | SS_CENTERIMAGE,
            left_x,
            ly,
            lbl_w,
            item_h,
            0,
            self.font_regular,
        );
        add_ctrl(
            &cls_btn,
            self.draft.method.as_upper(),
            WS_TABSTOP | BS_OWNERDRAW as u32,
            left_x + col_w - ctrl_w,
            ly,
            ctrl_w,
            item_h,
            ID_METHOD_BTN,
            self.font_bold,
        );
        ly += item_step;

        let numeric_rows = [
            ("History Count:", self.draft.hist.clone(), ID_EDIT_HIST),
            ("Loss Window:", self.draft.loss.clone(), ID_EDIT_LOSS),
            ("Avg Window:", self.draft.avg.clone(), ID_EDIT_AVG),
            ("Interval (ms):", self.draft.refresh.clone(), ID_EDIT_REF),
            ("Timeout (ms):", self.draft.timeout.clone(), ID_EDIT_TIMEOUT),
        ];

        for (label, val, id) in numeric_rows {
            add_ctrl(
                &cls_static,
                label,
                SS_LEFT | SS_CENTERIMAGE,
                left_x,
                ly,
                lbl_w,
                item_h,
                0,
                self.font_regular,
            );
            let h = add_ctrl(
                &cls_edit,
                &val,
                WS_TABSTOP | WS_BORDER | ES_CENTER | ES_AUTOHSCROLL,
                left_x + col_w - ctrl_w,
                ly,
                ctrl_w,
                item_h,
                id,
                self.font_regular,
            );
            match id {
                ID_EDIT_HIST => self.edit_hist = h,
                ID_EDIT_LOSS => self.edit_loss = h,
                ID_EDIT_AVG => self.edit_avg = h,
                ID_EDIT_REF => self.edit_ref = h,
                ID_EDIT_TIMEOUT => self.edit_timeout = h,
                _ => {}
            }
            ly += item_step;
        }

        // Right column: 5 toggles + Wave Speed
        let mut ry = y;
        let toggles = [
            ("Always on Top:", self.draft.always_on_top, ID_TOP_BTN),
            ("Save Position:", self.draft.save_position, ID_SAVE_POS_BTN),
            ("Autostart:", self.draft.autostart, ID_AUTO_BTN),
            ("Auto Update:", self.draft.check_updates, ID_CHECK_UPD_BTN),
            ("Wave Effect:", self.draft.wave_effect, ID_WAVE_BTN),
        ];

        for (label, on, id) in toggles {
            add_ctrl(
                &cls_static,
                label,
                SS_LEFT | SS_CENTERIMAGE,
                right_col_x,
                ry,
                lbl_w,
                item_h,
                0,
                self.font_regular,
            );
            add_ctrl(
                &cls_btn,
                if on { "ON" } else { "OFF" },
                WS_TABSTOP | BS_OWNERDRAW as u32,
                right_col_x + col_w - ctrl_w,
                ry,
                ctrl_w,
                item_h,
                id,
                self.font_bold,
            );
            ry += item_step;
        }

        add_ctrl(
            &cls_static,
            "Wave Speed:",
            SS_LEFT | SS_CENTERIMAGE,
            right_col_x,
            ry,
            lbl_w,
            item_h,
            0,
            self.font_regular,
        );
        add_ctrl(
            &cls_btn,
            self.draft.wave_speed.as_upper(),
            WS_TABSTOP | BS_OWNERDRAW as u32,
            right_col_x + col_w - ctrl_w,
            ry,
            ctrl_w,
            item_h,
            ID_SPEED_BTN,
            self.font_bold,
        );

        y = ly.max(ry + item_step) + px(4);

        // Paired steppers row: Font (left) + Opacity (right)
        let step_btn_w = px(20);
        let step_gap = px(2);
        let step_ctrl_w = px(76);
        let step_edit_w = step_ctrl_w - step_btn_w * 2 - step_gap * 2;
        let step_lbl_w = col_w - step_ctrl_w - px(4);

        let lx_ctrl = left_x + col_w - step_ctrl_w;
        add_ctrl(
            &cls_static,
            "Font (9-28):",
            SS_LEFT | SS_CENTERIMAGE,
            left_x,
            y,
            step_lbl_w,
            item_h,
            0,
            self.font_regular,
        );
        add_ctrl(
            &cls_btn,
            "-",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            lx_ctrl,
            y,
            step_btn_w,
            item_h,
            ID_FONT_DEC_BTN,
            self.font_bold,
        );
        self.edit_font = add_ctrl(
            &cls_edit,
            &self.draft.font_size.clone(),
            WS_TABSTOP | WS_BORDER | ES_CENTER | ES_AUTOHSCROLL,
            lx_ctrl + step_btn_w + step_gap,
            y,
            step_edit_w,
            item_h,
            ID_EDIT_FONT,
            self.font_regular,
        );
        add_ctrl(
            &cls_btn,
            "+",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            lx_ctrl + step_btn_w + step_gap + step_edit_w + step_gap,
            y,
            step_btn_w,
            item_h,
            ID_FONT_INC_BTN,
            self.font_bold,
        );

        let rx_ctrl = right_col_x + col_w - step_ctrl_w;
        add_ctrl(
            &cls_static,
            "Opacity (%):",
            SS_LEFT | SS_CENTERIMAGE,
            right_col_x,
            y,
            step_lbl_w,
            item_h,
            0,
            self.font_regular,
        );
        add_ctrl(
            &cls_btn,
            "-",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            rx_ctrl,
            y,
            step_btn_w,
            item_h,
            ID_OPACITY_DEC_BTN,
            self.font_bold,
        );
        self.edit_opacity = add_ctrl(
            &cls_edit,
            &self.draft.opacity.clone(),
            WS_TABSTOP | WS_BORDER | ES_CENTER | ES_AUTOHSCROLL,
            rx_ctrl + step_btn_w + step_gap,
            y,
            step_edit_w,
            item_h,
            ID_EDIT_OPACITY,
            self.font_regular,
        );
        add_ctrl(
            &cls_btn,
            "+",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            rx_ctrl + step_btn_w + step_gap + step_edit_w + step_gap,
            y,
            step_btn_w,
            item_h,
            ID_OPACITY_INC_BTN,
            self.font_bold,
        );
        y += item_step + px(4);

        // Inline Ping Colors row
        let cl_w = px(54);
        let c_gap = px(6);
        let c_w = (content_w - cl_w - c_gap * 3) / 4;
        let c_h = px(22);
        add_ctrl(
            &cls_static,
            "Colors:",
            SS_LEFT | SS_CENTERIMAGE,
            pad,
            y,
            cl_w,
            c_h,
            0,
            self.font_bold,
        );
        let color_btns = [
            ("Low", ID_COLOR_LOW_BTN),
            ("Med", ID_COLOR_MED_BTN),
            ("High", ID_COLOR_HIGH_BTN),
            ("T/O", ID_COLOR_TO_BTN),
        ];
        for (i, (label, id)) in color_btns.iter().enumerate() {
            let cx = pad + cl_w + (i as i32) * (c_w + c_gap);
            add_ctrl(
                &cls_btn,
                label,
                WS_TABSTOP | BS_OWNERDRAW as u32,
                cx,
                y,
                c_w,
                c_h,
                *id,
                self.font_bold,
            );
        }
        y += c_h + px(12);

        // Bottom 3 buttons: DEFAULTS | OPEN INI | SAVE
        let b_gap = px(8);
        let b_w = (content_w - b_gap * 2) / 3;
        let b_h = px(25);
        add_ctrl(
            &cls_btn,
            "DEFAULTS",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            pad,
            y,
            b_w,
            b_h,
            ID_DEFAULTS_BTN,
            self.font_bold,
        );
        add_ctrl(
            &cls_btn,
            "OPEN INI",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            pad + b_w + b_gap,
            y,
            b_w,
            b_h,
            ID_OPEN_INI_BTN,
            self.font_bold,
        );
        add_ctrl(
            &cls_btn,
            "SAVE",
            WS_TABSTOP | BS_OWNERDRAW as u32,
            pad + (b_w + b_gap) * 2,
            y,
            b_w,
            b_h,
            ID_SAVE_BTN,
            self.font_bold,
        );
        y += b_h + px(12);

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: y,
        };
        AdjustWindowRectEx(&mut r, style, 0, WS_EX_DLGMODALFRAME);
        let win_w = r.right - r.left;
        let win_h = r.bottom - r.top;

        if center_on_parent {
            let mut pr: RECT = std::mem::zeroed();
            GetWindowRect(self.parent, &mut pr);
            let mon = get_monitor_work_area(self.parent);

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

            SetWindowPos(self.hwnd, ptr::null_mut(), x, top_y, win_w, win_h, SWP_NOZORDER);
        } else {
            let mut cur: RECT = std::mem::zeroed();
            GetWindowRect(self.hwnd, &mut cur);
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                cur.left,
                cur.top,
                win_w,
                win_h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        InvalidateRect(self.hwnd, ptr::null(), 1);
        self.building_ui = false;
    }

    unsafe fn draw_color_pill(&self, dis: &DRAWITEMSTRUCT, label: &str, swatch_color: u32) {
        let pressed = (dis.itemState & ODS_SELECTED) != 0;
        let hdc = dis.hDC;
        FillRect(hdc, &dis.rcItem, self.brush_bg);

        let bg = if pressed {
            rgb(0x14, 0x14, 0x14)
        } else {
            rgb(0x1c, 0x1c, 0x1c)
        };
        let brush = CreateSolidBrush(bg);
        let pen = CreatePen(PS_SOLID as i32, 1, rgb(0x30, 0x30, 0x30));
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

        let sw = scale_px(10, self.scale);
        let sw_x = dis.rcItem.left + scale_px(7, self.scale);
        let sw_y = dis.rcItem.top + ((dis.rcItem.bottom - dis.rcItem.top) - sw) / 2;
        let sw_brush = CreateSolidBrush(swatch_color);
        let sw_pen = CreatePen(PS_SOLID as i32, 1, rgb(0x44, 0x44, 0x44));
        let ob = SelectObject(hdc, sw_brush as _);
        let op = SelectObject(hdc, sw_pen as _);
        RoundRect(
            hdc,
            sw_x,
            sw_y,
            sw_x + sw,
            sw_y + sw,
            scale_px(2, self.scale),
            scale_px(2, self.scale),
        );
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        DeleteObject(sw_brush as _);
        DeleteObject(sw_pen as _);

        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, rgb(0xdd, 0xdd, 0xdd));
        SelectObject(hdc, self.font_bold as _);
        let wide = to_wide(label);
        let mut tr = RECT {
            left: sw_x + sw + scale_px(5, self.scale),
            top: dis.rcItem.top,
            right: dis.rcItem.right - scale_px(4, self.scale),
            bottom: dis.rcItem.bottom,
        };
        DrawTextW(
            hdc,
            wide.as_ptr(),
            -1,
            &mut tr,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
    }

    unsafe fn draw_owner_button(&self, dis: &DRAWITEMSTRUCT) {
        let id = dis.CtlID as u16;

        match id {
            ID_COLOR_LOW_BTN => {
                self.draw_color_pill(dis, "Low", self.draft.colors.low);
                return;
            }
            ID_COLOR_MED_BTN => {
                self.draw_color_pill(dis, "Med", self.draft.colors.medium);
                return;
            }
            ID_COLOR_HIGH_BTN => {
                self.draw_color_pill(dis, "High", self.draft.colors.high);
                return;
            }
            ID_COLOR_TO_BTN => {
                self.draw_color_pill(dis, "T/O", self.draft.colors.timeout);
                return;
            }
            _ => {}
        }

        let pressed = (dis.itemState & ODS_SELECTED) != 0;

        let (text, bg, fg): (String, u32, u32) = match id {
            ID_ABOUT_BTN => (
                "ABOUT".to_string(),
                rgb(0x1f, 0x1f, 0x1f),
                rgb(0x00, 0xcc, 0xff),
            ),
            ID_ADD_TARGET => (
                "+ Add".to_string(),
                rgb(0x1c, 0x1c, 0x1c),
                rgb(0x00, 0xff, 0x66),
            ),
            ID_METHOD_BTN => (
                self.draft.method.as_upper().to_string(),
                rgb(0x22, 0x22, 0x22),
                method_color(self.draft.method),
            ),
            ID_TOP_BTN => toggle_style(self.draft.always_on_top),
            ID_SAVE_POS_BTN => toggle_style(self.draft.save_position),
            ID_AUTO_BTN => toggle_style(self.draft.autostart),
            ID_CHECK_UPD_BTN => toggle_style(self.draft.check_updates),
            ID_WAVE_BTN => toggle_style(self.draft.wave_effect),
            ID_SPEED_BTN => (
                self.draft.wave_speed.as_upper().to_string(),
                rgb(0x22, 0x22, 0x22),
                speed_color(self.draft.wave_speed),
            ),
            ID_FONT_DEC_BTN | ID_OPACITY_DEC_BTN => {
                ("-".to_string(), rgb(0x24, 0x24, 0x24), rgb(0xff, 0xff, 0xff))
            }
            ID_FONT_INC_BTN | ID_OPACITY_INC_BTN => {
                ("+".to_string(), rgb(0x24, 0x24, 0x24), rgb(0xff, 0xff, 0xff))
            }
            ID_DEFAULTS_BTN => (
                "DEFAULTS".to_string(),
                rgb(0xcc, 0x33, 0x33),
                rgb(0xff, 0xff, 0xff),
            ),
            ID_OPEN_INI_BTN => (
                "OPEN INI".to_string(),
                rgb(0x44, 0x44, 0x44),
                rgb(0xff, 0xff, 0xff),
            ),
            ID_SAVE_BTN => (
                "SAVE".to_string(),
                rgb(0x00, 0xaa, 0x44),
                rgb(0xff, 0xff, 0xff),
            ),
            x if x >= ID_TARGET_DEL_BASE => (
                "X".to_string(),
                rgb(0x1b, 0x1b, 0x1b),
                rgb(0xff, 0x44, 0x44),
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
        let pen = CreatePen(PS_SOLID as i32, 1, rgb(0x2c, 0x2c, 0x2c));
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

    unsafe fn try_save(&mut self) {
        self.sync_edits_to_draft();

        let targets: Vec<String> = self
            .draft
            .targets
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if targets.is_empty() {
            let msg = to_wide("Targets list cannot be empty.");
            let cap = to_wide("Error");
            MessageBoxW(self.hwnd, msg.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONERROR);
            return;
        }

        let parsed = (|| -> Option<(usize, usize, usize, u64, u64, u32, u8)> {
            let hist = self.draft.hist.trim().parse::<usize>().ok()?.clamp(1, 20);
            let loss = self.draft.loss.trim().parse::<usize>().ok()?.max(1);
            let avg = self.draft.avg.trim().parse::<usize>().ok()?.max(1);
            let refresh = self.draft.refresh.trim().parse::<u64>().ok()?.max(50);
            let timeout = self.draft.timeout.trim().parse::<u64>().ok()?.max(100);
            let font_size = self.draft.font_size.trim().parse::<u32>().ok()?.clamp(9, 28);
            let opacity = self.draft.opacity.trim().parse::<u8>().ok()?.clamp(25, 100);
            Some((hist, loss, avg, refresh, timeout, font_size, opacity))
        })();

        let Some((
            history_count,
            loss_window_size,
            avg_window_size,
            refresh_interval_ms,
            timeout_ms,
            font_size,
            opacity,
        )) = parsed
        else {
            let msg = to_wide("Please enter valid numeric values.");
            let cap = to_wide("Error");
            MessageBoxW(self.hwnd, msg.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONERROR);
            return;
        };

        let mut cfg = self.base_config.clone();
        cfg.targets = targets;
        cfg.ping_method = self.draft.method;
        cfg.history_count = history_count;
        cfg.loss_window_size = loss_window_size;
        cfg.avg_window_size = avg_window_size;
        cfg.refresh_interval_ms = refresh_interval_ms;
        cfg.timeout_ms = timeout_ms;
        cfg.always_on_top = self.draft.always_on_top;
        cfg.save_position = self.draft.save_position;
        cfg.autostart = self.draft.autostart;
        cfg.check_updates = self.draft.check_updates;
        cfg.wave_effect = self.draft.wave_effect;
        cfg.wave_speed = self.draft.wave_speed;
        cfg.font_size = font_size;
        cfg.opacity = opacity;
        cfg.colors = self.draft.colors;

        self.result = Some(cfg);
        self.close();
    }

    unsafe fn close(&mut self) {
        self.closed = true;
        EnableWindow(self.parent, 1);
        SetFocus(self.parent);
        DestroyWindow(self.hwnd);
    }
}

fn toggle_style(on: bool) -> (String, u32, u32) {
    if on {
        ("ON".to_string(), rgb(0x00, 0xaa, 0x44), rgb(0xff, 0xff, 0xff))
    } else {
        ("OFF".to_string(), rgb(0x3a, 0x3a, 0x3a), rgb(0xaa, 0xaa, 0xaa))
    }
}

unsafe extern "system" fn settings_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
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
            SetTextColor(hdc, rgb(0xbb, 0xbb, 0xbb));
            SetBkColor(hdc, BG_BOX);
            state.brush_bg as LRESULT
        }
        WM_CTLCOLOREDIT => {
            let hdc = wparam as HDC;
            SetTextColor(hdc, rgb(0xff, 0xff, 0xff));
            SetBkColor(hdc, rgb(0x22, 0x22, 0x22));
            state.brush_edit as LRESULT
        }
        WM_DRAWITEM => {
            let dis = &*(lparam as *const DRAWITEMSTRUCT);
            state.draw_owner_button(dis);
            1
        }
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as u16;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            if code == EN_CHANGE_CODE {
                state.emit_live_preview();
            } else if code == BN_CLICKED {
                match id {
                    ID_ABOUT_BTN => {
                        show_about_dialog(hwnd);
                    }
                    ID_ADD_TARGET => {
                        state.sync_edits_to_draft();
                        state.draft.targets.push(String::new());
                        state.rebuild_ui(false);
                        state.emit_live_preview();
                    }
                    ID_METHOD_BTN => {
                        state.draft.method = state.draft.method.next();
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                        state.emit_live_preview();
                    }
                    ID_TOP_BTN => {
                        state.draft.always_on_top = !state.draft.always_on_top;
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                        state.emit_live_preview();
                    }
                    ID_SAVE_POS_BTN => {
                        state.draft.save_position = !state.draft.save_position;
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                        state.emit_live_preview();
                    }
                    ID_AUTO_BTN => {
                        state.draft.autostart = !state.draft.autostart;
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                    }
                    ID_CHECK_UPD_BTN => {
                        state.draft.check_updates = !state.draft.check_updates;
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                    }
                    ID_WAVE_BTN => {
                        state.draft.wave_effect = !state.draft.wave_effect;
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                        state.emit_live_preview();
                    }
                    ID_SPEED_BTN => {
                        state.draft.wave_speed = state.draft.wave_speed.next();
                        InvalidateRect(lparam as HWND, ptr::null(), 0);
                        state.emit_live_preview();
                    }
                    ID_FONT_DEC_BTN => {
                        state.step_font_size(-1);
                    }
                    ID_FONT_INC_BTN => {
                        state.step_font_size(1);
                    }
                    ID_OPACITY_DEC_BTN => {
                        state.step_opacity(-5);
                    }
                    ID_OPACITY_INC_BTN => {
                        state.step_opacity(5);
                    }
                    ID_COLOR_LOW_BTN => {
                        if let Some(c) = pick_color_dialog(hwnd, state.draft.colors.low) {
                            state.draft.colors.low = c;
                            InvalidateRect(lparam as HWND, ptr::null(), 0);
                            state.emit_live_preview();
                        }
                    }
                    ID_COLOR_MED_BTN => {
                        if let Some(c) = pick_color_dialog(hwnd, state.draft.colors.medium) {
                            state.draft.colors.medium = c;
                            InvalidateRect(lparam as HWND, ptr::null(), 0);
                            state.emit_live_preview();
                        }
                    }
                    ID_COLOR_HIGH_BTN => {
                        if let Some(c) = pick_color_dialog(hwnd, state.draft.colors.high) {
                            state.draft.colors.high = c;
                            InvalidateRect(lparam as HWND, ptr::null(), 0);
                            state.emit_live_preview();
                        }
                    }
                    ID_COLOR_TO_BTN => {
                        if let Some(c) = pick_color_dialog(hwnd, state.draft.colors.timeout) {
                            state.draft.colors.timeout = c;
                            InvalidateRect(lparam as HWND, ptr::null(), 0);
                            state.emit_live_preview();
                        }
                    }
                    ID_DEFAULTS_BTN => {
                        state.draft = Draft::from_config(&Config::default());
                        state.rebuild_ui(false);
                        state.emit_live_preview();
                    }
                    ID_OPEN_INI_BTN => {
                        open_file_in_shell(&config_path());
                    }
                    ID_SAVE_BTN => {
                        state.try_save();
                    }
                    x if x >= ID_TARGET_DEL_BASE => {
                        let idx = (x - ID_TARGET_DEL_BASE) as usize;
                        state.sync_edits_to_draft();
                        if idx < state.draft.targets.len() {
                            state.draft.targets.remove(idx);
                            state.rebuild_ui(false);
                            state.emit_live_preview();
                        }
                    }
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

pub fn show_settings_dialog(parent: HWND, current: &Config) -> Option<Config> {
    unsafe {
        let hinst = GetModuleHandleW(ptr::null());
        let class_name = to_wide("NanoPingSettingsWindow");
        let brush_bg = CreateSolidBrush(BG_BOX);
        let brush_edit = CreateSolidBrush(rgb(0x22, 0x22, 0x22));

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(settings_wnd_proc),
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

        let title = to_wide(&format!("Settings — {APP_NAME} v{APP_VERSION}"));
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            0,
            0,
            320,
            520,
            parent,
            ptr::null_mut(),
            hinst,
            ptr::null(),
        );
        if hwnd.is_null() {
            DeleteObject(brush_bg as _);
            DeleteObject(brush_edit as _);
            return None;
        }

        enable_dark_title_bar(hwnd);
        apply_exe_icon(hwnd);

        let scale = get_dpi_scale(hwnd);
        let segoe = to_wide("Segoe UI");
        let font_regular = CreateFontW(
            -scale_px(13, scale),
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
        let font_bold = CreateFontW(
            -scale_px(12, scale),
            0,
            0,
            0,
            700,
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

        let mut state = Box::new(SettingsState {
            hwnd,
            parent,
            scale,
            draft: Draft::from_config(current),
            base_config: current.clone(),
            result: None,
            closed: false,
            building_ui: false,
            font_regular,
            font_bold,
            brush_bg,
            brush_edit,
            child_hwnds: Vec::new(),
            target_edits: Vec::new(),
            edit_hist: ptr::null_mut(),
            edit_loss: ptr::null_mut(),
            edit_avg: ptr::null_mut(),
            edit_ref: ptr::null_mut(),
            edit_timeout: ptr::null_mut(),
            edit_font: ptr::null_mut(),
            edit_opacity: ptr::null_mut(),
        });

        let state_ptr = &mut *state as *mut SettingsState;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

        EnableWindow(parent, 0);
        state.rebuild_ui(true);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while !state.closed && GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        DeleteObject(font_regular as _);
        DeleteObject(font_bold as _);
        DeleteObject(brush_bg as _);
        DeleteObject(brush_edit as _);

        state.result
    }
}