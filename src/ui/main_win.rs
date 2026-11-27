use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetDC,
    GetTextExtentPoint32W, InvalidateRect, ReleaseDC, RoundRect, ScreenToClient, SelectObject,
    SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HFONT, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetWindowLongPtrW, GetWindowRect, IsIconic, KillTimer,
    LoadCursorW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassExW, SetCursor, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, GWLP_USERDATA, HTCAPTION, HTCLIENT, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW,
    IDC_HAND, IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_YESNO, MSG, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MINIMIZE, SW_SHOW, WM_APP, WM_CLOSE, WM_DESTROY,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST,
    WM_NCLBUTTONDBLCLK, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_TOPMOST, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
};

use crate::config::{set_autostart, Config};
use crate::ping::{PingEngine, PingEvent};
use crate::state::AppStats;
use crate::ui::settings_win::show_settings_dialog;
use crate::ui::tooltip::DarkTooltip;
use crate::ui::win_util::{
    apply_exe_icon, apply_window_opacity, enable_borderless_frame, get_dpi_scale, method_color,
    resolve_safe_window_position, rgb, scale_px, to_wide, APP_NAME, APP_VERSION, BG_BOX, BG_BTN,
    BG_BTN_HOVER, BG_MAIN,
};
use crate::updater::{
    check_for_updates, download_and_install_update, UpdateInfo, UpdateStatus,
};

pub const WM_APP_PING_READY: u32 = WM_APP + 1;
pub const WM_APP_PREVIEW_CONFIG: u32 = WM_APP + 3;
const WM_APP_UPDATE_EVENT: u32 = WM_APP + 4;
const WM_DISPLAYCHANGE: u32 = 0x007E;
const ANIM_TIMER_ID: usize = 1;
const TOOLTIP_TIMER_ID: usize = 2;
const TOOLTIP_DELAY_MS: u32 = 450;
const WM_SETTINGCHANGE: u32 = 0x001A;
const SPI_SETWORKAREA: usize = 0x002F;
const SETTLE_TIMER_ID: usize = 3;

const WINDOW_STYLE: u32 = WS_POPUP | WS_MINIMIZEBOX | WS_SYSMENU;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TopBtn {
    Update,
    Set,
    Pin,
    Method,
    Reset,
    Minimize,
    Close,
}

struct ButtonRect {
    kind: TopBtn,
    rect: RECT,
}

enum MainUpdateEvent {
    Available(UpdateInfo),
    InstallDone(Result<(), String>),
}

struct Layout {
    client_w: i32,
    client_h: i32,
    pad_x: i32,
    top_h: i32,
    row_h: i32,
    ip_w: i32,
    loss_w: i32,
    avg_w: i32,
    hist_cell_w: i32,
    sep_w: i32,
    gap: i32,
    buttons: Vec<ButtonRect>,
}

struct MainState {
    hwnd: HWND,
    config: Config,
    stats: AppStats,
    engine: PingEngine,
    tx: Sender<PingEvent>,
    rx: Receiver<PingEvent>,
    upd_tx: Sender<MainUpdateEvent>,
    upd_rx: Receiver<MainUpdateEvent>,
    available_update: Option<UpdateInfo>,
    updating: bool,
    scale: f32,
    font_ui: HFONT,
    font_mono: HFONT,
    font_hist: HFONT,
    font_sep: HFONT,
    layout: Layout,
    hovered_btn: Option<TopBtn>,
    tracking_mouse: bool,
    anim_timer_active: bool,
    tooltip: DarkTooltip,
}

impl MainState {
    unsafe fn create_fonts(scale: f32, font_size: u32) -> (HFONT, HFONT, HFONT, HFONT) {
        let segoe = to_wide("Segoe UI");
        let consolas = to_wide("Consolas");
        let base = font_size.clamp(9, 28) as f32;

        let h_mono = base.round() as i32;
        let h_hist = (base * 0.86).round().max(8.0) as i32;
        let h_ui = (base * 0.79).round().max(8.0) as i32;
        let h_sep = (base * 0.72).round().max(7.0) as i32;

        let mk = |h: i32, weight: i32, face: &[u16]| -> HFONT {
            CreateFontW(
                -scale_px(h, scale),
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
                face.as_ptr(),
            )
        };

        (
            mk(h_ui, 700, &segoe),
            mk(h_mono, 700, &consolas),
            mk(h_hist, 700, &consolas),
            mk(h_sep, 400, &consolas),
        )
    }

    unsafe fn recreate_fonts(&mut self) {
        DeleteObject(self.font_ui as _);
        DeleteObject(self.font_mono as _);
        DeleteObject(self.font_hist as _);
        DeleteObject(self.font_sep as _);
        let (f_ui, f_mono, f_hist, f_sep) = Self::create_fonts(self.scale, self.config.font_size);
        self.font_ui = f_ui;
        self.font_mono = f_mono;
        self.font_hist = f_hist;
        self.font_sep = f_sep;
    }

    unsafe fn measure_char_width(hwnd: HWND, font: HFONT, sample: &str) -> i32 {
        let hdc = GetDC(hwnd);
        let old = SelectObject(hdc, font as _);
        let wide = to_wide(sample);
        let mut sz = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(hdc, wide.as_ptr(), (wide.len() - 1) as i32, &mut sz);
        SelectObject(hdc, old);
        ReleaseDC(hwnd, hdc);
        sz.cx
    }

    unsafe fn compute_layout(
        hwnd: HWND,
        cfg: &Config,
        scale: f32,
        font_mono: HFONT,
        font_hist: HFONT,
        font_ui: HFONT,
        show_update_btn: bool,
    ) -> Layout {
        let ratio = (cfg.font_size.clamp(9, 28) as f32) / 14.0;
        let s = scale * ratio;

        let pad_x = scale_px(7, s);
        let pad_bottom = scale_px(5, s);
        let top_h = scale_px(26, s);
        let row_h = scale_px(21, s);
        let gap = scale_px(4, s);

        let max_ip_chars = cfg
            .targets
            .iter()
            .map(|t| t.len())
            .max()
            .unwrap_or(8)
            .max(8);
        let sample_ip = "8".repeat(max_ip_chars);
        let ip_w = Self::measure_char_width(hwnd, font_mono, &sample_ip) + scale_px(2, s);
        let loss_w = Self::measure_char_width(hwnd, font_mono, "L:99%") + scale_px(6, s);
        let avg_w = Self::measure_char_width(hwnd, font_mono, "A:999ms") + scale_px(6, s);
        let hist_cell_w = Self::measure_char_width(hwnd, font_hist, "999ms") + scale_px(2, s);
        let sep_w = scale_px(5, s);

        let hist_total_w = if cfg.history_count == 0 {
            0
        } else {
            (cfg.history_count as i32) * hist_cell_w
                + ((cfg.history_count - 1) as i32) * sep_w
        };

        let table_w = ip_w + gap + loss_w + gap + avg_w + gap + hist_total_w;

        let title_w = Self::measure_char_width(hwnd, font_ui, "NANOPING (ICMP)");
        let btn_pad_x = scale_px(5, s);
        let btn_gap = scale_px(3, s);

        let mut btn_labels: Vec<(TopBtn, &str)> = Vec::with_capacity(7);
        if show_update_btn {
            btn_labels.push((TopBtn::Update, "UPD"));
        }
        btn_labels.push((TopBtn::Set, "SET"));
        btn_labels.push((TopBtn::Pin, "PIN"));
        btn_labels.push((TopBtn::Method, "ICMP"));
        btn_labels.push((TopBtn::Reset, "RST"));
        btn_labels.push((TopBtn::Minimize, "\u{2212}"));
        btn_labels.push((TopBtn::Close, "\u{00D7}"));

        let min_ctrl_w = scale_px(18, s);
        let ctrl_sep = scale_px(5, s);
        let mut btn_widths = Vec::with_capacity(btn_labels.len());
        let mut total_btns_w = ctrl_sep;
        for (kind, text) in btn_labels {
            let mut w = Self::measure_char_width(hwnd, font_ui, text) + btn_pad_x * 2;
            if kind == TopBtn::Minimize || kind == TopBtn::Close {
                w = w.max(min_ctrl_w);
            }
            btn_widths.push((kind, w));
            total_btns_w += w;
        }
        if btn_widths.len() > 1 {
            total_btns_w += btn_gap * (btn_widths.len() as i32 - 1);
        }

        let min_header_w = title_w + scale_px(12, s) + total_btns_w;
        let content_w = table_w.max(min_header_w);

        let client_w = pad_x * 2 + content_w;
        let client_h = top_h + (cfg.targets.len() as i32) * row_h + pad_bottom;

        let btn_top = scale_px(4, s);
        let btn_bot = top_h - scale_px(3, s);
        let mut right_cursor = client_w - pad_x;
        let mut buttons = Vec::with_capacity(btn_widths.len());
        for (kind, w) in btn_widths.into_iter().rev() {
            let rect = RECT {
                left: right_cursor - w,
                top: btn_top,
                right: right_cursor,
                bottom: btn_bot,
            };
            buttons.push(ButtonRect { kind, rect });
            let extra = if kind == TopBtn::Minimize { ctrl_sep } else { 0 };
            right_cursor -= w + btn_gap + extra;
        }
        buttons.reverse();

        Layout {
            client_w,
            client_h,
            pad_x,
            top_h,
            row_h,
            ip_w,
            loss_w,
            avg_w,
            hist_cell_w,
            sep_w,
            gap,
            buttons,
        }
    }

    unsafe fn refresh_layout(&mut self) {
        self.layout = Self::compute_layout(
            self.hwnd,
            &self.config,
            self.scale,
            self.font_mono,
            self.font_hist,
            self.font_ui,
            self.available_update.is_some(),
        );
        self.apply_size_and_position(false);
    }

    unsafe fn calc_window_size(&self) -> (i32, i32) {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: self.layout.client_w,
            bottom: self.layout.client_h,
        };
        let ex_style = if self.config.always_on_top {
            WS_EX_TOPMOST
        } else {
            0
        };
        AdjustWindowRectEx(&mut r, WINDOW_STYLE, 0, ex_style);
        (r.right - r.left, r.bottom - r.top)
    }

    unsafe fn apply_size_and_position(&mut self, use_saved_pos: bool) {
        let (w, h) = self.calc_window_size();
        if use_saved_pos {
            let (sx, sy) = if self.config.save_position {
                (self.config.window_x, self.config.window_y)
            } else {
                (None, None)
            };
            let (x, y, _) = resolve_safe_window_position(sx, sy, w, h);
            let z = if self.config.always_on_top {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            };
            SetWindowPos(self.hwnd, z, x, y, w, h, SWP_NOACTIVATE);
        } else {
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    unsafe fn ensure_on_connected_monitor(&mut self) {
        if IsIconic(self.hwnd) != 0 {
            return;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(self.hwnd, &mut rect) == 0 {
            return;
        }
        let (w, h) = self.calc_window_size();
        let (target_x, target_y) = if self.config.save_position {
            (
                self.config.window_x.or(Some(rect.left)),
                self.config.window_y.or(Some(rect.top)),
            )
        } else {
            (Some(rect.left), Some(rect.top))
        };
        let (x, y, _) = resolve_safe_window_position(target_x, target_y, w, h);
        if rect.left != x || rect.top != y {
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                x,
                y,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    unsafe fn save_current_position_immediately(&mut self) {
        if !self.config.save_position || IsIconic(self.hwnd) != 0 {
            return;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(self.hwnd, &mut rect) != 0 {
            self.config.window_x = Some(rect.left);
            self.config.window_y = Some(rect.top);
            self.config.save();
        }
    }

    unsafe fn start_or_restart_pingers(&mut self) {
        self.engine.bump_reset_cycle();
        self.engine.set_method(self.config.ping_method);
        self.stats = AppStats::from_config(&self.config);

        self.refresh_layout();
        self.update_title(APP_NAME);
        InvalidateRect(self.hwnd, ptr::null(), 0);

        self.engine.start_workers(
            self.hwnd,
            &self.config.targets,
            self.config.refresh_interval_ms,
            self.config.timeout_ms,
            self.tx.clone(),
        );
    }

    unsafe fn apply_config_change(&mut self, mut new_cfg: Config, persist: bool) {
        if new_cfg.save_position && IsIconic(self.hwnd) == 0 {
            let mut rect: RECT = std::mem::zeroed();
            if GetWindowRect(self.hwnd, &mut rect) != 0 {
                new_cfg.window_x = Some(rect.left);
                new_cfg.window_y = Some(rect.top);
            }
        }

        let needs_rebuild = new_cfg.targets != self.config.targets
            || new_cfg.history_count != self.config.history_count
            || new_cfg.ping_method != self.config.ping_method
            || new_cfg.refresh_interval_ms != self.config.refresh_interval_ms
            || new_cfg.timeout_ms != self.config.timeout_ms;

        let window_sizes_changed = new_cfg.loss_window_size != self.config.loss_window_size
            || new_cfg.avg_window_size != self.config.avg_window_size;

        let topmost_changed = new_cfg.always_on_top != self.config.always_on_top;
        let font_changed = new_cfg.font_size != self.config.font_size;
        let opacity_changed = new_cfg.opacity != self.config.opacity;

        if persist {
            new_cfg.save();
            set_autostart(new_cfg.autostart);
        }
        self.config = new_cfg;

        if font_changed {
            self.recreate_fonts();
        }
        if opacity_changed {
            apply_window_opacity(self.hwnd, self.config.opacity);
        }

        if topmost_changed {
            let z = if self.config.always_on_top {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            };
            SetWindowPos(
                self.hwnd,
                z,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }

        if needs_rebuild {
            self.start_or_restart_pingers();
        } else if window_sizes_changed {
            if font_changed {
                self.refresh_layout();
            }
            self.stats = AppStats::from_config(&self.config);
            self.reset_stats();
        } else {
            if font_changed {
                self.refresh_layout();
            }
            InvalidateRect(self.hwnd, ptr::null(), 0);
        }
    }

    unsafe fn start_update_install(&mut self, info: UpdateInfo) {
        if self.updating {
            return;
        }
        self.updating = true;
        InvalidateRect(self.hwnd, ptr::null(), 0);

        let tx = self.upd_tx.clone();
        let hwnd_raw = self.hwnd as isize;
        thread::spawn(move || {
            let res = download_and_install_update(&info.download_url);
            if tx.send(MainUpdateEvent::InstallDone(res)).is_ok() {
                let _ = PostMessageW(hwnd_raw as HWND, WM_APP_UPDATE_EVENT, 0, 0);
            }
        });
    }

    unsafe fn reset_stats(&mut self) {
        self.engine.bump_reset_cycle();
        self.stats.reset();
        self.update_title(APP_NAME);
        InvalidateRect(self.hwnd, ptr::null(), 0);
    }

    unsafe fn update_title(&self, title: &str) {
        let wide = to_wide(title);
        SetWindowTextW(self.hwnd, wide.as_ptr());
    }

    unsafe fn ensure_anim_timer(&mut self) {
        if !self.anim_timer_active {
            SetTimer(self.hwnd, ANIM_TIMER_ID, 16, None);
            self.anim_timer_active = true;
        }
    }

    unsafe fn stop_anim_timer(&mut self) {
        if self.anim_timer_active {
            KillTimer(self.hwnd, ANIM_TIMER_ID);
            self.anim_timer_active = false;
        }
    }
    fn tooltip_text(&self, kind: TopBtn) -> String {
        match kind {
            TopBtn::Update => {
                if self.updating {
                    "Downloading & installing update...".to_string()
                } else if let Some(ref info) = self.available_update {
                    let tag = if info.tag.starts_with(['v', 'V']) {
                        info.tag.clone()
                    } else {
                        format!("v{}", info.tag)
                    };
                    format!("Update Available ({tag})")
                } else {
                    "Update Available".to_string()
                }
            }
            TopBtn::Set => "Settings".to_string(),
            TopBtn::Pin => {
                if self.config.always_on_top {
                    "Always on Top: ON".to_string()
                } else {
                    "Always on Top: OFF".to_string()
                }
            }
            TopBtn::Method => format!(
                "Ping Method: {} (Click to switch)",
                self.config.ping_method.as_upper()
            ),
            TopBtn::Reset => "Reset Statistics".to_string(),
            TopBtn::Minimize => "Minimize".to_string(),
            TopBtn::Close => "Close".to_string(),
        }
    }

    unsafe fn cancel_tooltip(&self) {
        KillTimer(self.hwnd, TOOLTIP_TIMER_ID);
        self.tooltip.hide();
    }

    fn hit_test_button(&self, x: i32, y: i32) -> Option<TopBtn> {
        for b in &self.layout.buttons {
            if x >= b.rect.left && x <= b.rect.right && y >= b.rect.top && y <= b.rect.bottom {
                return Some(b.kind);
            }
        }
        None
    }

    unsafe fn handle_quick_button(&mut self, kind: TopBtn) {
        match kind {
            TopBtn::Pin => {
                self.config.always_on_top = !self.config.always_on_top;
                let z = if self.config.always_on_top {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                };
                SetWindowPos(self.hwnd, z, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                self.config.save();
                InvalidateRect(self.hwnd, ptr::null(), 0);
            }
            TopBtn::Method => {
                self.config.ping_method = self.config.ping_method.next();
                self.engine.set_method(self.config.ping_method);
                self.config.save();
                self.reset_stats();
            }
            TopBtn::Reset => {
                self.reset_stats();
            }
            TopBtn::Minimize => {
                ShowWindow(self.hwnd, SW_MINIMIZE);
            }
            TopBtn::Close => {
                let _ = PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
            TopBtn::Set | TopBtn::Update => {}
        }
    }

    unsafe fn paint(&mut self) {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(self.hwnd, &mut ps);

        let mut cr: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut cr);
        let w = (cr.right - cr.left).max(1);
        let h = (cr.bottom - cr.top).max(1);

        let mem_dc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem_dc, bmp as _);

        let bg_brush = CreateSolidBrush(BG_MAIN);
        FillRect(mem_dc, &cr, bg_brush);
        DeleteObject(bg_brush as _);

        SetBkMode(mem_dc, TRANSPARENT as i32);

        let l = &self.layout;
        let now = Instant::now();

        // 1. Header title
        SelectObject(mem_dc, self.font_ui as _);
        SetTextColor(mem_dc, rgb(0x55, 0x55, 0x55));
        let header_text = to_wide(&format!("NANOPING ({})", self.config.ping_method.as_upper()));
        let mut header_rect = RECT {
            left: l.pad_x,
            top: 0,
            right: l.client_w - l.pad_x,
            bottom: l.top_h,
        };
        DrawTextW(
            mem_dc,
            header_text.as_ptr(),
            -1,
            &mut header_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        // 2. Header buttons
        let corner = scale_px(4, self.scale);
        let pen_btn_border = CreatePen(PS_SOLID as i32, 1, rgb(0x22, 0x22, 0x22));
        let pen_upd_border = CreatePen(PS_SOLID as i32, 1, rgb(0x00, 0x88, 0xaa));
        for btn in &l.buttons {
            let hovered = self.hovered_btn == Some(btn.kind);
            let fill_col = if btn.kind == TopBtn::Update {
                if hovered {
                    rgb(0x18, 0x3a, 0x4e)
                } else {
                    rgb(0x0e, 0x24, 0x32)
                }
            } else if btn.kind == TopBtn::Close && hovered {
                rgb(0xc4, 0x2b, 0x1c)
            } else if hovered {
                BG_BTN_HOVER
            } else {
                BG_BTN
            };
            let fill = CreateSolidBrush(fill_col);
            let border_pen = if btn.kind == TopBtn::Update {
                pen_upd_border
            } else {
                pen_btn_border
            };
            let old_b = SelectObject(mem_dc, fill as _);
            let old_p = SelectObject(mem_dc, border_pen as _);
            RoundRect(
                mem_dc,
                btn.rect.left,
                btn.rect.top,
                btn.rect.right,
                btn.rect.bottom,
                corner,
                corner,
            );
            SelectObject(mem_dc, old_b);
            SelectObject(mem_dc, old_p);
            DeleteObject(fill as _);

            let (label, fg) = match btn.kind {
                TopBtn::Update => (
                    if self.updating { "..." } else { "UPD" },
                    if hovered {
                        rgb(0xff, 0xff, 0xff)
                    } else {
                        rgb(0x00, 0xdd, 0xff)
                    },
                ),
                TopBtn::Set => (
                    "SET",
                    if hovered {
                        rgb(0xff, 0xff, 0xff)
                    } else {
                        rgb(0xaa, 0xaa, 0xaa)
                    },
                ),
                TopBtn::Pin => (
                    "PIN",
                    if self.config.always_on_top {
                        rgb(0x00, 0xff, 0x66)
                    } else if hovered {
                        rgb(0xaa, 0xaa, 0xaa)
                    } else {
                        rgb(0x66, 0x66, 0x66)
                    },
                ),
                TopBtn::Method => (
                    self.config.ping_method.as_upper(),
                    method_color(self.config.ping_method),
                ),
                TopBtn::Reset => (
                    "RST",
                    if hovered {
                        rgb(0xff, 0xff, 0xff)
                    } else {
                        rgb(0xff, 0xcc, 0x00)
                    },
                ),
                TopBtn::Minimize => (
                    "\u{2212}",
                    if hovered {
                        rgb(0xff, 0xff, 0xff)
                    } else {
                        rgb(0x88, 0x88, 0x88)
                    },
                ),
                TopBtn::Close => (
                    "\u{00D7}",
                    if hovered {
                        rgb(0xff, 0xff, 0xff)
                    } else {
                        rgb(0x99, 0x99, 0x99)
                    },
                ),
            };
            SetTextColor(mem_dc, fg);
            let wide = to_wide(label);
            let mut r = btn.rect;
            DrawTextW(
                mem_dc,
                wide.as_ptr(),
                -1,
                &mut r,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
        DeleteObject(pen_btn_border as _);
        DeleteObject(pen_upd_border as _);

        // 3. Target rows
        let box_brush = CreateSolidBrush(BG_BOX);
        let box_pen = CreatePen(PS_SOLID as i32, 1, rgb(0x1b, 0x1b, 0x1b));
        let box_pad_y = scale_px(2, self.scale);

        for (row_idx, row) in self.stats.rows.iter().enumerate() {
            let y_top = l.top_h + (row_idx as i32) * l.row_h;
            let y_bot = y_top + l.row_h;
            let mut x = l.pad_x;

            SelectObject(mem_dc, self.font_mono as _);
            SetTextColor(mem_dc, rgb(0xff, 0xff, 0xff));
            let mut r_ip = RECT {
                left: x,
                top: y_top,
                right: x + l.ip_w,
                bottom: y_bot,
            };
            let w_ip = to_wide(&row.target);
            DrawTextW(
                mem_dc,
                w_ip.as_ptr(),
                -1,
                &mut r_ip,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE,
            );
            x += l.ip_w + l.gap;

            // Loss box
            let mut r_loss = RECT {
                left: x,
                top: y_top + box_pad_y,
                right: x + l.loss_w,
                bottom: y_bot - box_pad_y,
            };
            let old_b = SelectObject(mem_dc, box_brush as _);
            let old_p = SelectObject(mem_dc, box_pen as _);
            RoundRect(
                mem_dc,
                r_loss.left,
                r_loss.top,
                r_loss.right,
                r_loss.bottom,
                corner,
                corner,
            );
            SelectObject(mem_dc, old_b);
            SelectObject(mem_dc, old_p);

            SetTextColor(mem_dc, row.loss_color(&self.config.colors));
            let w_loss = to_wide(&format!("L:{:.0}%", row.loss_pct()));
            DrawTextW(
                mem_dc,
                w_loss.as_ptr(),
                -1,
                &mut r_loss,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            x += l.loss_w + l.gap;

            // Average box
            let mut r_avg = RECT {
                left: x,
                top: y_top + box_pad_y,
                right: x + l.avg_w,
                bottom: y_bot - box_pad_y,
            };
            let old_b = SelectObject(mem_dc, box_brush as _);
            let old_p = SelectObject(mem_dc, box_pen as _);
            RoundRect(
                mem_dc,
                r_avg.left,
                r_avg.top,
                r_avg.right,
                r_avg.bottom,
                corner,
                corner,
            );
            SelectObject(mem_dc, old_b);
            SelectObject(mem_dc, old_p);

            SetTextColor(mem_dc, row.avg_color(&self.config.colors));
            let w_avg = to_wide(&format!("A:{}ms", row.avg_ms()));
            DrawTextW(
                mem_dc,
                w_avg.as_ptr(),
                -1,
                &mut r_avg,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            x += l.avg_w + l.gap;

            // History cells
            for i in 0..row.history.len() {
                if i > 0 {
                    SelectObject(mem_dc, self.font_sep as _);
                    SetTextColor(mem_dc, rgb(0x28, 0x28, 0x28));
                    let mut r_sep = RECT {
                        left: x,
                        top: y_top,
                        right: x + l.sep_w,
                        bottom: y_bot,
                    };
                    let sep = to_wide("|");
                    DrawTextW(
                        mem_dc,
                        sep.as_ptr(),
                        -1,
                        &mut r_sep,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    x += l.sep_w;
                }

                SelectObject(mem_dc, self.font_hist as _);
                let (col, _) =
                    row.cell_render_color(i, self.config.wave_speed, &self.config.colors, now);
                SetTextColor(mem_dc, col);
                let mut r_cell = RECT {
                    left: x,
                    top: y_top,
                    right: x + l.hist_cell_w,
                    bottom: y_bot,
                };
                let w_val = to_wide(&row.history[i].text);
                DrawTextW(
                    mem_dc,
                    w_val.as_ptr(),
                    -1,
                    &mut r_cell,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
                x += l.hist_cell_w;
            }
        }

        DeleteObject(box_brush as _);
        DeleteObject(box_pen as _);

        let frame_brush = CreateSolidBrush(rgb(0x24, 0x24, 0x24));
        FrameRect(mem_dc, &cr, frame_brush);
        DeleteObject(frame_brush as _);

        BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
        SelectObject(mem_dc, old_bmp);
        DeleteObject(bmp as _);
        DeleteDC(mem_dc);

        EndPaint(self.hwnd, &ps);
    }
}

impl Drop for MainState {
    fn drop(&mut self) {
        self.engine.stop();
        unsafe {
            DeleteObject(self.font_ui as _);
            DeleteObject(self.font_mono as _);
            DeleteObject(self.font_hist as _);
            DeleteObject(self.font_sep as _);
        }
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MainState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    match msg {
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => {
            let mut pt = POINT {
                x: (lparam & 0xFFFF) as i16 as i32,
                y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
            };
            ScreenToClient(hwnd, &mut pt);
            if (*state_ptr).hit_test_button(pt.x, pt.y).is_some() {
                HTCLIENT as LRESULT
            } else {
                HTCAPTION as LRESULT
            }
        }
        WM_NCLBUTTONDBLCLK => 0,
        WM_PAINT => {
            (*state_ptr).paint();
            0
        }
        WM_APP_PING_READY => {
            let state = &mut *state_ptr;
            let active_cycle = state.engine.current_cycle();
            let mut any_updated = false;
            while let Ok(ev) = state.rx.try_recv() {
                if ev.cycle != active_cycle {
                    continue;
                }
                if let Some(row) = state.stats.rows.get_mut(ev.target_idx) {
                    row.push(ev.result, state.config.wave_effect);
                    any_updated = true;
                }
            }
            if any_updated {
                let title = state.stats.build_window_title();
                state.update_title(&title);
                if state.config.wave_effect {
                    state.ensure_anim_timer();
                }
                InvalidateRect(hwnd, ptr::null(), 0);
            }
            0
        }
        WM_APP_PREVIEW_CONFIG => {
            let cfg_ptr = lparam as *const Config;
            if !cfg_ptr.is_null() {
                let preview = (*cfg_ptr).clone();
                (*state_ptr).apply_config_change(preview, false);
            }
            0
        }
        WM_APP_UPDATE_EVENT => {
            let mut install_error: Option<String> = None;
            {
                let state = &mut *state_ptr;
                while let Ok(ev) = state.upd_rx.try_recv() {
                    match ev {
                        MainUpdateEvent::Available(info) => {
                            state.available_update = Some(info);
                            state.refresh_layout();
                            InvalidateRect(hwnd, ptr::null(), 0);
                        }
                        MainUpdateEvent::InstallDone(Ok(())) => {
                            std::process::exit(0);
                        }
                        MainUpdateEvent::InstallDone(Err(err)) => {
                            state.updating = false;
                            InvalidateRect(hwnd, ptr::null(), 0);
                            install_error = Some(err);
                        }
                    }
                }
            }
            if let Some(err) = install_error {
                let msg = to_wide(&format!("Update failed:\n{err}"));
                let cap = to_wide(&format!("{APP_NAME} — Update Error"));
                MessageBoxW(hwnd, msg.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONERROR);
            }
            0
        }
        WM_TIMER => {
            let state = &mut *state_ptr;
            if wparam == ANIM_TIMER_ID {
                let active = state
                    .stats
                    .has_active_animations(state.config.wave_speed, Instant::now());
                InvalidateRect(hwnd, ptr::null(), 0);
                if !active {
                    state.stop_anim_timer();
                }
            } else if wparam == TOOLTIP_TIMER_ID {
                KillTimer(hwnd, TOOLTIP_TIMER_ID);
                if let Some(kind) = state.hovered_btn {
                        if let Some(btn) = state.layout.buttons.iter().find(|b| b.kind == kind) {
                            let text = state.tooltip_text(kind);
                            state.tooltip.show(hwnd, btn.rect, &text);
                        }
                    }
                } else if wparam == SETTLE_TIMER_ID {
                    KillTimer(hwnd, SETTLE_TIMER_ID);
                    state.ensure_on_connected_monitor();
                }
                0
            }

        WM_MOUSEMOVE => {
            let state = &mut *state_ptr;
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            if !state.tracking_mouse {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
                state.tracking_mouse = true;
            }
            let hovered = state.hit_test_button(x, y);
            if hovered != state.hovered_btn {
                state.hovered_btn = hovered;
                state.cancel_tooltip();
                if hovered.is_some() {
                    SetTimer(hwnd, TOOLTIP_TIMER_ID, TOOLTIP_DELAY_MS, None);
                }
                InvalidateRect(hwnd, ptr::null(), 0);
            }
            0
        }
        WM_MOUSELEAVE => {
            let state = &mut *state_ptr;
            state.tracking_mouse = false;
            if state.hovered_btn.is_some() {
                state.hovered_btn = None;
                InvalidateRect(hwnd, ptr::null(), 0);
            }
            state.cancel_tooltip();
            0
        
        }
        WM_SETCURSOR => {
            if (lparam & 0xFFFF) == 1 && (*state_ptr).hovered_btn.is_some() {
                SetCursor(LoadCursorW(ptr::null_mut(), IDC_HAND));
                return 1;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            (*state_ptr).cancel_tooltip();
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let clicked = (*state_ptr).hit_test_button(x, y);
            if let Some(btn) = clicked {
                match btn {
                    TopBtn::Set => {
                        let original_cfg = (*state_ptr).config.clone();
                        let saved = show_settings_dialog(hwnd, &original_cfg);
                        let state = &mut *state_ptr;
                        match saved {
                            Some(new_cfg) => state.apply_config_change(new_cfg, true),
                            None => state.apply_config_change(original_cfg, false),
                        }
                    }
                    TopBtn::Update => {
                        if !(*state_ptr).updating {
                            if let Some(info) = (*state_ptr).available_update.clone() {
                                let display_tag = if info.tag.starts_with(['v', 'V']) {
                                    info.tag.clone()
                                } else {
                                    format!("v{}", info.tag)
                                };
                                let prompt = to_wide(&format!(
                                    "A new version of {APP_NAME} ({display_tag}) is available.\n\
                                     Current version: v{APP_VERSION}\n\n\
                                     Do you want to download, replace the current executable, and restart {APP_NAME} now?\n\
                                     (Your settings in config.ini will be preserved.)"
                                ));
                                let caption = to_wide(&format!("Update Available — {APP_NAME}"));
                                let choice = MessageBoxW(
                                    hwnd,
                                    prompt.as_ptr(),
                                    caption.as_ptr(),
                                    MB_YESNO | MB_ICONQUESTION,
                                );
                                if choice == IDYES {
                                    (*state_ptr).start_update_install(info);
                                }
                            }
                        }
                    }
                    _ => (*state_ptr).handle_quick_button(btn),
                }
            }
            0
        }
        WM_EXITSIZEMOVE => {
            KillTimer(hwnd, SETTLE_TIMER_ID);
            (*state_ptr).cancel_tooltip();
            (*state_ptr).save_current_position_immediately();
            0
        }
        WM_DISPLAYCHANGE => {
            (*state_ptr).cancel_tooltip();
            (*state_ptr).ensure_on_connected_monitor();
            0
        }
        WM_SETTINGCHANGE => {
            if wparam == SPI_SETWORKAREA {
                (*state_ptr).ensure_on_connected_monitor();
            }
            0
        }
        WM_CLOSE => {
            KillTimer(hwnd, SETTLE_TIMER_ID);
            (*state_ptr).cancel_tooltip();
            let state = &mut *state_ptr;
            state.stop_anim_timer();
            state.engine.stop();
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let _ = Box::from_raw(state_ptr);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn run() {
    let config = Config::load();
    let stats = AppStats::from_config(&config);
    let engine = PingEngine::new(config.ping_method);
    let (tx, rx) = mpsc::channel();
    let (upd_tx, upd_rx) = mpsc::channel();

    unsafe {
        let hinst = GetModuleHandleW(ptr::null());
        let class_name = to_wide("NanoPingMainWindow");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(main_wnd_proc),
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

        let ex_style = WS_EX_LAYERED | if config.always_on_top { WS_EX_TOPMOST } else { 0 };
        let title = to_wide(APP_NAME);
        let hwnd = CreateWindowExW(
            ex_style,
            class_name.as_ptr(),
            title.as_ptr(),
            WINDOW_STYLE,
            0,
            0,
            300,
            120,
            ptr::null_mut(),
            ptr::null_mut(),
            hinst,
            ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }

        enable_borderless_frame(hwnd);
        apply_exe_icon(hwnd);
        apply_window_opacity(hwnd, config.opacity);

        let scale = get_dpi_scale(hwnd);
        let (font_ui, font_mono, font_hist, font_sep) =
            MainState::create_fonts(scale, config.font_size);
        let layout = MainState::compute_layout(
            hwnd,
            &config,
            scale,
            font_mono,
            font_hist,
            font_ui,
            false,
        );

        let bg_upd_tx = upd_tx.clone();
        let mut state = Box::new(MainState {
            hwnd,
            config,
            stats,
            engine,
            tx,
            rx,
            upd_tx,
            upd_rx,
            available_update: None,
            updating: false,
            scale,
            font_ui,
            font_mono,
            font_hist,
            font_sep,
            layout,
            hovered_btn: None,
            tracking_mouse: false,
            anim_timer_active: false,
            tooltip: DarkTooltip::new(hwnd, scale),
        });

        state.apply_size_and_position(true);

        let state_ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        // Re-verify saved position once desktop/monitors finish settling after Windows boot
        if (*state_ptr).config.save_position
            && (*state_ptr).config.window_x.is_some()
            && (*state_ptr).config.window_y.is_some()
        {
            SetTimer(hwnd, SETTLE_TIMER_ID, 2500, None);
        }

        // Start ping worker threads
        let s = &mut *state_ptr;
        s.engine.start_workers(
            hwnd,
            &s.config.targets,
            s.config.refresh_interval_ms,
            s.config.timeout_ms,
            s.tx.clone(),
        );

        // Silent background update check shortly after launch
        let hwnd_raw = hwnd as isize;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1500));
            if let UpdateStatus::Available(info) = check_for_updates() {
                if bg_upd_tx.send(MainUpdateEvent::Available(info)).is_ok() {
                    let _ = PostMessageW(hwnd_raw as HWND, WM_APP_UPDATE_EVENT, 0, 0);
                }
            }
        });

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}