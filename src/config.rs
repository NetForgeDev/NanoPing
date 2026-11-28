use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ,
};

use crate::ui::win_util::{format_hex_color, parse_hex_color, rgb, to_wide};

const DEFAULT_HEADER: &str = "\
; Configuration File for NanoPing
; ping_method: icmp (Native Windows API), tcp (Port 443), or udp (DNS/QUIC/STUN/Port Probe)
; wave_speed: normal, slow, fast, very fast
; wave_style: off, pulse, fade, slide
; targets syntax: host[|method] or s5://[user:pass@]host:port[|method] or http://[user:pass@]host:port

";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingMethod {
    Icmp = 0,
    Tcp = 1,
    Udp = 2,
}

impl PingMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Icmp => "icmp",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }

    pub fn as_upper(self) -> &'static str {
        match self {
            Self::Icmp => "ICMP",
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "tcp" => Self::Tcp,
            "udp" => Self::Udp,
            _ => Self::Icmp,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Tcp,
            2 => Self::Udp,
            _ => Self::Icmp,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Icmp => Self::Tcp,
            Self::Tcp => Self::Udp,
            Self::Udp => Self::Icmp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveSpeed {
    Normal,
    Slow,
    Fast,
    VeryFast,
}

impl WaveSpeed {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Slow => "slow",
            Self::Fast => "fast",
            Self::VeryFast => "very fast",
        }
    }

    pub fn as_upper(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Slow => "SLOW",
            Self::Fast => "FAST",
            Self::VeryFast => "VERY FAST",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "slow" => Self::Slow,
            "fast" => Self::Fast,
            "very fast" => Self::VeryFast,
            _ => Self::Normal,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Slow,
            Self::Slow => Self::Fast,
            Self::Fast => Self::VeryFast,
            Self::VeryFast => Self::Normal,
        }
    }

    pub fn timing_ms(self) -> (u64, u64) {
        match self {
            Self::Normal => (40, 180),
            Self::Slow => (60, 260),
            Self::Fast => (25, 120),
            Self::VeryFast => (15, 75),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveStyle {
    Off,
    Pulse,
    Fade,
    Slide,
}

impl WaveStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pulse => "pulse",
            Self::Fade => "fade",
            Self::Slide => "slide",
        }
    }

    pub fn as_upper(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Pulse => "PULSE",
            Self::Fade => "FADE",
            Self::Slide => "SLIDE",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" => Self::Off,
            "fade" => Self::Fade,
            "slide" => Self::Slide,
            _ => Self::Pulse,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Pulse,
            Self::Pulse => Self::Fade,
            Self::Fade => Self::Slide,
            Self::Slide => Self::Off,
        }
    }

    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }

    pub fn color(self) -> u32 {
        match self {
            Self::Off => rgb(0xaa, 0xaa, 0xaa),
            Self::Pulse => rgb(0x00, 0xdd, 0xff),
            Self::Fade => rgb(0x00, 0xff, 0x66),
            Self::Slide => rgb(0xcc, 0x77, 0xff),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingColors {
    pub low: u32,
    pub medium: u32,
    pub high: u32,
    pub timeout: u32,
}

impl Default for PingColors {
    fn default() -> Self {
        Self {
            low: rgb(0x00, 0xcc, 0x55),
            medium: rgb(0xe6, 0xe6, 0x00),
            high: rgb(0xe6, 0x8a, 0x00),
            timeout: rgb(0xcc, 0x33, 0x33),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Direct,
    Socks5,
    Http,
}

impl TargetKind {
    pub fn as_badge(self) -> &'static str {
        match self {
            Self::Direct => "IP",
            Self::Socks5 => "S5",
            Self::Http => "HT",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Direct => Self::Socks5,
            Self::Socks5 => Self::Http,
            Self::Http => Self::Direct,
        }
    }

    pub fn color(self) -> u32 {
        match self {
            Self::Direct => rgb(0x88, 0x88, 0x88),
            Self::Socks5 => rgb(0x00, 0xdd, 0xff),
            Self::Http => rgb(0xcc, 0x77, 0xff),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetEntry {
    pub address: String,
    pub kind: TargetKind,
    /// `None` = AUTO (inherits global ping method); `Some(m)` = per-target override.
    pub method: Option<PingMethod>,
}

impl TargetEntry {
    pub fn direct(addr: &str) -> Self {
        Self {
            address: addr.to_string(),
            kind: TargetKind::Direct,
            method: None,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }

        let (addr_part, mut method) = if let Some((left, right)) = trimmed.rsplit_once('|') {
            match right.trim().to_ascii_lowercase().as_str() {
                "icmp" => (left.trim(), Some(PingMethod::Icmp)),
                "tcp" => (left.trim(), Some(PingMethod::Tcp)),
                "udp" => (left.trim(), Some(PingMethod::Udp)),
                "auto" => (left.trim(), None),
                _ => (trimmed, None),
            }
        } else {
            (trimmed, None)
        };

        let lower = addr_part.to_ascii_lowercase();
        let (kind, address) = if lower.starts_with("socks5://") {
            (TargetKind::Socks5, addr_part[9..].trim())
        } else if lower.starts_with("s5://") {
            (TargetKind::Socks5, addr_part[5..].trim())
        } else if lower.starts_with("http://") {
            (TargetKind::Http, addr_part[7..].trim())
        } else if lower.starts_with("ht://") {
            (TargetKind::Http, addr_part[5..].trim())
        } else {
            (TargetKind::Direct, addr_part.trim())
        };

        if address.is_empty() {
            return None;
        }

        match kind {
            TargetKind::Http => method = Some(PingMethod::Tcp),
            TargetKind::Socks5 if method == Some(PingMethod::Icmp) => {
                method = Some(PingMethod::Tcp);
            }
            _ => {}
        }

        Some(Self {
            address: address.to_string(),
            kind,
            method,
        })
    }

    pub fn to_config_str(&self) -> String {
        let prefix = match self.kind {
            TargetKind::Direct => "",
            TargetKind::Socks5 => "s5://",
            TargetKind::Http => "http://",
        };
        let suffix = match (self.kind, self.method) {
            (TargetKind::Http, _) | (_, None) => String::new(),
            (_, Some(m)) => format!("|{}", m.as_str()),
        };
        format!("{prefix}{}{suffix}", self.address)
    }

    /// Returns display text for the main HUD, hiding optional `user:pass@` proxy credentials.
    pub fn display_label(&self) -> &str {
        if self.kind != TargetKind::Direct {
            if let Some((_, host_port)) = self.address.rsplit_once('@') {
                if !host_port.is_empty() {
                    return host_port;
                }
            }
        }
        &self.address
    }

    /// Splits optional `user:pass@host:port` proxy address into `(Option<(user, pass)>, host_port)`.
    pub fn split_proxy_auth(&self) -> (Option<(&str, &str)>, &str) {
        if let Some((creds, host_port)) = self.address.rsplit_once('@') {
            if let Some((user, pass)) = creds.split_once(':') {
                return (Some((user, pass)), host_port);
            }
            return (Some((creds, "")), host_port);
        }
        (None, &self.address)
    }

    pub fn effective_method(&self, global: PingMethod) -> PingMethod {
        match self.kind {
            TargetKind::Http => PingMethod::Tcp,
            TargetKind::Socks5 => match self.method {
                Some(PingMethod::Udp) => PingMethod::Udp,
                Some(_) => PingMethod::Tcp,
                None => {
                    if global == PingMethod::Udp {
                        PingMethod::Udp
                    } else {
                        PingMethod::Tcp
                    }
                }
            },
            TargetKind::Direct => self.method.unwrap_or(global),
        }
    }

    pub fn is_overridden(&self) -> bool {
        self.kind != TargetKind::Http && self.method.is_some()
    }

    pub fn cycle_method(&mut self) {
        self.method = match self.kind {
            TargetKind::Direct => match self.method {
                None => Some(PingMethod::Icmp),
                Some(PingMethod::Icmp) => Some(PingMethod::Tcp),
                Some(PingMethod::Tcp) => Some(PingMethod::Udp),
                Some(PingMethod::Udp) => None,
            },
            TargetKind::Socks5 => match self.method {
                None => Some(PingMethod::Tcp),
                Some(PingMethod::Tcp) => Some(PingMethod::Udp),
                _ => None,
            },
            TargetKind::Http => Some(PingMethod::Tcp),
        };
    }

    /// Cycles directly to the next distinct method on main-window clicks,
    /// snapping back to `None` (AUTO) when wrapping to the global default.
    pub fn cycle_method_for_global(&mut self, global: PingMethod) {
        if self.kind == TargetKind::Http {
            self.method = Some(PingMethod::Tcp);
            return;
        }
        let cur = self.effective_method(global);
        let next = match self.kind {
            TargetKind::Direct => cur.next(),
            TargetKind::Socks5 => match cur {
                PingMethod::Udp => PingMethod::Tcp,
                _ => PingMethod::Udp,
            },
            TargetKind::Http => PingMethod::Tcp,
        };
        self.method = Some(next);
    }

    pub fn method_label(&self) -> &'static str {
        match self.kind {
            TargetKind::Http => "TCP",
            _ => match self.method {
                None => "AUTO",
                Some(m) => m.as_upper(),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub targets: Vec<TargetEntry>,
    pub always_on_top: bool,
    pub history_count: usize,
    pub loss_window_size: usize,
    pub avg_window_size: usize,
    pub refresh_interval_ms: u64,
    pub timeout_ms: u64,
    pub ping_method: PingMethod,
    pub save_position: bool,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub autostart: bool,
    pub wave_style: WaveStyle,
    pub wave_speed: WaveSpeed,
    pub font_size: u32,
    pub opacity: u8,
    pub check_updates: bool,
    pub proxy_probe_host: String,
    pub colors: PingColors,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            targets: vec![
                TargetEntry::direct("8.8.8.8"),
                TargetEntry::direct("4.2.2.4"),
                TargetEntry::direct("google.com"),
            ],
            always_on_top: true,
            history_count: 3,
            loss_window_size: 20,
            avg_window_size: 15,
            refresh_interval_ms: 1000,
            timeout_ms: 2000,
            ping_method: PingMethod::Icmp,
            save_position: true,
            window_x: None,
            window_y: None,
            autostart: false,
            wave_style: WaveStyle::Pulse,
            wave_speed: WaveSpeed::Normal,
            font_size: 14,
            opacity: 100,
            check_updates: true,
            proxy_probe_host: "1.1.1.1".to_string(),
            colors: PingColors::default(),
        }
    }
}

fn local_config_path() -> PathBuf {
    #[cfg(debug_assertions)]
    {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join("config.ini");
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("config.ini")
}

fn appdata_config_path() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let p = PathBuf::from(appdata).join("NanoPing").join("config.ini");
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    local_config_path()
}

/// Returns `true` when using portable `config.ini` beside the executable.
pub fn is_portable_mode() -> bool {
    local_config_path().exists()
}

/// Auto-detects portable mode (local `config.ini` beside `.exe`) vs standard Windows `%APPDATA%\NanoPing\config.ini`.
pub fn config_path() -> PathBuf {
    let local = local_config_path();
    if local.exists() {
        local
    } else {
        appdata_config_path()
    }
}

/// Migrates `config.ini` between Portable (beside `.exe`) and standard AppData (`%APPDATA%\NanoPing\`).
pub fn switch_config_storage(cfg: &Config, portable: bool) -> Result<(), &'static str> {
    let local = local_config_path();
    let appdata = appdata_config_path();
    if local == appdata {
        cfg.save();
        return Ok(());
    }

    let content = cfg.format_ini();
    if portable {
        fs::write(&local, content)
            .map_err(|_| "Cannot write config.ini beside NanoPing.exe (check folder permissions).")?;
        if appdata.exists() {
            let _ = fs::remove_file(&appdata);
            if let Some(dir) = appdata.parent() {
                let _ = fs::remove_dir(dir);
            }
        }
    } else {
        if let Some(dir) = appdata.parent() {
            fs::create_dir_all(dir)
                .map_err(|_| "Cannot create %APPDATA%\\NanoPing directory.")?;
        }
        fs::write(&appdata, content)
            .map_err(|_| "Cannot write config.ini to %APPDATA%\\NanoPing.")?;
        if local.exists() {
            let _ = fs::remove_file(&local);
        }
    }
    Ok(())
}

fn parse_bool(val: &str, fallback: bool) -> bool {
    match val.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => fallback,
    }
}

fn fmt_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if !path.exists() {
            let cfg = Self::default();
            cfg.save();
            return cfg;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };

        let mut map = HashMap::new();
        let mut in_settings = true;

        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                in_settings = line[1..line.len() - 1].eq_ignore_ascii_case("SETTINGS");
                continue;
            }
            if in_settings {
                if let Some((k, v)) = line.split_once('=') {
                    map.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
        }

        let defaults = Self::default();
        let expected_keys = [
            "targets",
            "always_on_top",
            "history_count",
            "loss_window_size",
            "avg_window_size",
            "refresh_interval_ms",
            "timeout_ms",
            "ping_method",
            "save_position",
            "window_x",
            "window_y",
            "autostart",
            "wave_style",
            "wave_speed",
            "font_size",
            "color_low",
            "color_medium",
            "color_high",
            "color_timeout",
            "opacity",
            "check_updates",
            "proxy_probe_host",
        ];
        let needs_rewrite = expected_keys.iter().any(|k| !map.contains_key(*k));

        let targets: Vec<TargetEntry> = map
            .get("targets")
            .map(|s| s.split(',').filter_map(TargetEntry::parse).collect())
            .filter(|v: &Vec<TargetEntry>| !v.is_empty())
            .unwrap_or_else(|| defaults.targets.clone());

        let parse_pos = |key: &str| -> Option<i32> {
            map.get(key).and_then(|v| {
                if v.eq_ignore_ascii_case("center") {
                    None
                } else {
                    v.parse::<i32>().ok()
                }
            })
        };

        let colors = PingColors {
            low: map
                .get("color_low")
                .and_then(|v| parse_hex_color(v))
                .unwrap_or(defaults.colors.low),
            medium: map
                .get("color_medium")
                .and_then(|v| parse_hex_color(v))
                .unwrap_or(defaults.colors.medium),
            high: map
                .get("color_high")
                .and_then(|v| parse_hex_color(v))
                .unwrap_or(defaults.colors.high),
            timeout: map
                .get("color_timeout")
                .and_then(|v| parse_hex_color(v))
                .unwrap_or(defaults.colors.timeout),
        };

        let cfg = Self {
            targets,
            always_on_top: map
                .get("always_on_top")
                .map(|v| parse_bool(v, defaults.always_on_top))
                .unwrap_or(defaults.always_on_top),
            history_count: map
                .get("history_count")
                .and_then(|v| v.parse::<usize>().ok())
                .map(|v| v.clamp(1, 20))
                .unwrap_or(defaults.history_count),
            loss_window_size: map
                .get("loss_window_size")
                .and_then(|v| v.parse::<usize>().ok())
                .map(|v| v.max(1))
                .unwrap_or(defaults.loss_window_size),
            avg_window_size: map
                .get("avg_window_size")
                .and_then(|v| v.parse::<usize>().ok())
                .map(|v| v.max(1))
                .unwrap_or(defaults.avg_window_size),
            refresh_interval_ms: map
                .get("refresh_interval_ms")
                .and_then(|v| v.parse::<u64>().ok())
                .map(|v| v.max(50))
                .unwrap_or(defaults.refresh_interval_ms),
            timeout_ms: map
                .get("timeout_ms")
                .and_then(|v| v.parse::<u64>().ok())
                .map(|v| v.max(100))
                .unwrap_or(defaults.timeout_ms),
            ping_method: map
                .get("ping_method")
                .map(|v| PingMethod::from_str(v))
                .unwrap_or(defaults.ping_method),
            save_position: map
                .get("save_position")
                .map(|v| parse_bool(v, defaults.save_position))
                .unwrap_or(defaults.save_position),
            window_x: parse_pos("window_x"),
            window_y: parse_pos("window_y"),
            autostart: map
                .get("autostart")
                .map(|v| parse_bool(v, defaults.autostart))
                .unwrap_or(defaults.autostart),
            wave_style: map
                .get("wave_style")
                .map(|v| WaveStyle::from_str(v))
                .unwrap_or_else(|| {
                    if let Some(legacy) = map.get("wave_effect") {
                        if parse_bool(legacy, true) {
                            WaveStyle::Pulse
                        } else {
                            WaveStyle::Off
                        }
                    } else {
                        defaults.wave_style
                    }
                }),
            wave_speed: map
                .get("wave_speed")
                .map(|v| WaveSpeed::from_str(v))
                .unwrap_or(defaults.wave_speed),
            font_size: map
                .get("font_size")
                .and_then(|v| v.parse::<u32>().ok())
                .map(|v| v.clamp(9, 28))
                .unwrap_or(defaults.font_size),
            colors,
            opacity: map
                .get("opacity")
                .and_then(|v| v.parse::<u8>().ok())
                .map(|v| v.clamp(25, 100))
                .unwrap_or(defaults.opacity),
            check_updates: map
                .get("check_updates")
                .map(|v| parse_bool(v, defaults.check_updates))
                .unwrap_or(defaults.check_updates),
            proxy_probe_host: map
                .get("proxy_probe_host")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| defaults.proxy_probe_host.clone()),
        };

        if needs_rewrite {
            cfg.save();
        }
        cfg
    }

    fn format_ini(&self) -> String {
        let wx = self
            .window_x
            .map(|v| v.to_string())
            .unwrap_or_else(|| "center".to_string());
        let wy = self
            .window_y
            .map(|v| v.to_string())
            .unwrap_or_else(|| "center".to_string());

        format!(
            "{header}[SETTINGS]\n\
targets = {targets}\n\
always_on_top = {always_on_top}\n\
history_count = {history_count}\n\
loss_window_size = {loss_window_size}\n\
avg_window_size = {avg_window_size}\n\
refresh_interval_ms = {refresh_interval_ms}\n\
timeout_ms = {timeout_ms}\n\
ping_method = {ping_method}\n\
save_position = {save_position}\n\
window_x = {wx}\n\
window_y = {wy}\n\
autostart = {autostart}\n\
wave_style = {wave_style}\n\
wave_speed = {wave_speed}\n\
font_size = {font_size}\n\
opacity = {opacity}\n\
check_updates = {check_updates}\n\
proxy_probe_host = {proxy_probe_host}\n\
color_low = {color_low}\n\
color_medium = {color_medium}\n\
color_high = {color_high}\n\
color_timeout = {color_timeout}\n",
            header = DEFAULT_HEADER,
            targets = self
                .targets
                .iter()
                .map(|t| t.to_config_str())
                .collect::<Vec<_>>()
                .join(", "),
            always_on_top = fmt_bool(self.always_on_top),
            history_count = self.history_count,
            loss_window_size = self.loss_window_size,
            avg_window_size = self.avg_window_size,
            refresh_interval_ms = self.refresh_interval_ms,
            timeout_ms = self.timeout_ms,
            ping_method = self.ping_method.as_str(),
            save_position = fmt_bool(self.save_position),
            autostart = fmt_bool(self.autostart),
            wave_style = self.wave_style.as_str(),
            wave_speed = self.wave_speed.as_str(),
            font_size = self.font_size,
            opacity = self.opacity,
            check_updates = fmt_bool(self.check_updates),
            proxy_probe_host = self.proxy_probe_host,
            color_low = format_hex_color(self.colors.low),
            color_medium = format_hex_color(self.colors.medium),
            color_high = format_hex_color(self.colors.high),
            color_timeout = format_hex_color(self.colors.timeout),
        )
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, self.format_ini());
    }
}

impl Config {
    pub fn has_proxy_targets(&self) -> bool {
        self.targets.iter().any(|t| t.kind != TargetKind::Direct)
    }
}

pub fn set_autostart(enable: bool) {
    let subkey = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = to_wide("NanoPing");

    unsafe {
        let mut hkey: HKEY = ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        ) != 0
        {
            return;
        }

        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let quoted = format!("\"{}\"", exe.display());
                let wide = to_wide(&quoted);
                let byte_len = (wide.len() * std::mem::size_of::<u16>()) as u32;
                let _ = RegSetValueExW(
                    hkey,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    wide.as_ptr() as *const u8,
                    byte_len,
                );
            }
        } else {
            let _ = RegDeleteValueW(hkey, value_name.as_ptr());
        }

        let _ = RegCloseKey(hkey);
    }
}