use std::collections::HashMap;
use std::ffi::c_void;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::windows::io::AsRawSocket;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY,
};
use windows_sys::Win32::Networking::WinSock::WSAIoctl;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::PingMethod;
use crate::ui::main_win::WM_APP_PING_READY;

const SIO_UDP_CONNRESET: u32 = 0x9800_000C;
const WSAECONNRESET: i32 = 10054;
const IP_SUCCESS: u32 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingTier {
    Empty,
    Low,
    Medium,
    High,
    Timeout,
}

#[derive(Clone, Debug)]
pub struct PingResult {
    pub display: String,
    pub tier: PingTier,
    pub is_lost: bool,
    pub latency_ms: Option<u32>,
}

pub struct PingEvent {
    pub cycle: u64,
    pub target_idx: usize,
    pub result: PingResult,
}

pub fn format_latency(latency_ms: Option<u32>, timeout_ms: u64) -> PingResult {
    let Some(ms) = latency_ms else {
        return PingResult {
            display: "T/O".to_string(),
            tier: PingTier::Timeout,
            is_lost: true,
            latency_ms: None,
        };
    };

    if (ms as u64) >= timeout_ms {
        let display = if timeout_ms >= 1000 {
            if timeout_ms % 1000 == 0 {
                format!(">{}s", timeout_ms / 1000)
            } else {
                format!(">{:.1}s", timeout_ms as f64 / 1000.0).replace(".0s", "s")
            }
        } else {
            format!("{timeout_ms}+")
        };
        return PingResult {
            display,
            tier: PingTier::Timeout,
            is_lost: true,
            latency_ms: Some(timeout_ms as u32),
        };
    }

    let display = if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    };

    let tier = if ms < 100 {
        PingTier::Low
    } else if ms <= 200 {
        PingTier::Medium
    } else {
        PingTier::High
    };

    PingResult {
        display,
        tier,
        is_lost: false,
        latency_ms: Some(ms),
    }
}

fn resolve_ipv4(target: &str, cache: &mut HashMap<String, Ipv4Addr>) -> Option<Ipv4Addr> {
    if let Ok(ip) = target.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    if let Some(&ip) = cache.get(target) {
        return Some(ip);
    }
    let resolved = (target, 0u16)
        .to_socket_addrs()
        .ok()?
        .find_map(|addr| match addr.ip() {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        });
    if let Some(ip) = resolved {
        cache.insert(target.to_string(), ip);
    }
    resolved
}

/// Native Windows ICMP ping via `IcmpSendEcho` (iphlpapi.dll).
/// Requires NO Administrator privileges.
fn ping_icmp(ip: Ipv4Addr, timeout_ms: u64) -> Option<u32> {
    unsafe {
        let handle = IcmpCreateFile();
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return None;
        }

        let dest_addr = u32::from_ne_bytes(ip.octets());
        let payload = [0x61u8; 32];
        // 256-byte 8-byte-aligned reply buffer covers ICMP_ECHO_REPLY + payload + error bytes on both x86 and x64
        let mut reply_buf = [0u64; 32];

        let count = IcmpSendEcho(
            handle,
            dest_addr,
            payload.as_ptr() as *const c_void,
            payload.len() as u16,
            ptr::null(),
            reply_buf.as_mut_ptr() as *mut c_void,
            (reply_buf.len() * std::mem::size_of::<u64>()) as u32,
            timeout_ms as u32,
        );

        let result = if count > 0 {
            let reply = &*(reply_buf.as_ptr() as *const ICMP_ECHO_REPLY);
            if reply.Status == IP_SUCCESS {
                Some(reply.RoundTripTime)
            } else {
                None
            }
        } else {
            None
        };

        IcmpCloseHandle(handle);
        result
    }
}

/// TCP handshake latency probe on port 443.
fn ping_tcp(ip: Ipv4Addr, timeout_ms: u64) -> Option<u32> {
    let addr = SocketAddr::from((ip, 443));
    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => Some(start.elapsed().as_millis() as u32),
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
            // Host actively responded with TCP RST
            Some(start.elapsed().as_millis() as u32)
        }
        Err(_) => None,
    }
}

/// Builds a standards-compliant 1200-byte RFC 9000 QUIC Version Negotiation probe.
/// Modern QUIC/HTTP3 servers (Google, Cloudflare, etc.) reply immediately with Version Negotiation.
fn build_quic_probe(seed: u64) -> Vec<u8> {
    let mut pkt = vec![0u8; 1200];
    pkt[0] = 0xC0; // Long Header | Fixed Bit | Initial
    // Reserved "GREASE" version guaranteed to trigger Version Negotiation (RFC 9000 §6.3)
    pkt[1] = 0x0A;
    pkt[2] = 0x0A;
    pkt[3] = 0x0A;
    pkt[4] = 0x0A;
    pkt[5] = 8; // DCID length
    pkt[6..14].copy_from_slice(&seed.to_le_bytes());
    pkt[14] = 8; // SCID length
    pkt[15..23].copy_from_slice(&seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
    pkt
}

/// Multi-protocol real UDP probe:
/// 1. DNS standard query (port 53)
/// 2. QUIC Version Negotiation probe (port 443)
/// 3. STUN Binding Request (port 3478)
/// 4. Traceroute high-port probe (port 33434 -> triggers ICMP Port Unreachable / WSAECONNRESET)
fn ping_udp(ip: Ipv4Addr, timeout_ms: u64, seq: u64) -> Option<u32> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    let timeout = Duration::from_millis(timeout_ms);
    let _ = socket.set_read_timeout(Some(timeout));

    // Enable SIO_UDP_CONNRESET so ICMP Port Unreachable immediately raises WSAECONNRESET
    unsafe {
        let mut enable: i32 = 1;
        let mut bytes_returned: u32 = 0;
        let _ = WSAIoctl(
            socket.as_raw_socket() as usize,
            SIO_UDP_CONNRESET,
            &mut enable as *mut i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
            ptr::null_mut(),
            0,
            &mut bytes_returned,
            ptr::null_mut(),
            None,
        );
    }

    let tx_hi = ((seq >> 8) & 0xFF) as u8;
    let tx_lo = (seq & 0xFF) as u8;

    // 1. DNS query for a.root-servers.net (A record)
    let dns_query: [u8; 36] = [
        tx_hi, tx_lo, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, b'a',
        0x0c, b'r', b'o', b'o', b't', b'-', b's', b'e', b'r', b'v', b'e', b'r', b's', 0x03, b'n',
        b'e', b't', 0x00, 0x00, 0x01, 0x00, 0x01,
    ];

    // 2. QUIC Version Negotiation probe (RFC 9000)
    let quic_probe = build_quic_probe(seq);

    // 3. STUN Binding Request (RFC 5389)
    let mut stun_req = [0u8; 20];
    stun_req[0] = 0x00;
    stun_req[1] = 0x01; // Binding Request
    stun_req[4..8].copy_from_slice(&[0x21, 0x12, 0xA4, 0x42]); // Magic cookie
    stun_req[8..16].copy_from_slice(&seq.to_le_bytes());

    // 4. Traceroute-style closed-port probe
    let trace_probe = b"PINGMON";

    let start = Instant::now();
    let _ = socket.send_to(&dns_query, SocketAddr::from((ip, 53)));
    let _ = socket.send_to(&quic_probe, SocketAddr::from((ip, 443)));
    let _ = socket.send_to(&stun_req, SocketAddr::from((ip, 3478)));
    let _ = socket.send_to(trace_probe, SocketAddr::from((ip, 33434)));

    let mut buf = [0u8; 1500];
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return None;
        }
        let _ = socket.set_read_timeout(Some(timeout.saturating_sub(elapsed)));

        match socket.recv_from(&mut buf) {
            Ok((_, from)) => {
                if from.ip() == IpAddr::V4(ip) {
                    return Some(start.elapsed().as_millis() as u32);
                }
            }
            Err(e) if e.raw_os_error() == Some(WSAECONNRESET) => {
                let ms = start.elapsed().as_millis() as u32;
                if (ms as u64) < timeout_ms {
                    return Some(ms);
                }
                return None;
            }
            Err(_) => return None,
        }
    }
}

pub struct PingEngine {
    running: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    reset_cycle: Arc<AtomicU64>,
    method: Arc<AtomicU8>,
}

impl PingEngine {
    pub fn new(initial_method: PingMethod) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            generation: Arc::new(AtomicU64::new(0)),
            reset_cycle: Arc::new(AtomicU64::new(0)),
            method: Arc::new(AtomicU8::new(initial_method as u8)),
        }
    }

    pub fn set_method(&self, method: PingMethod) {
        self.method.store(method as u8, Ordering::Relaxed);
    }

    pub fn bump_reset_cycle(&self) -> u64 {
        self.reset_cycle.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_cycle(&self) -> u64 {
        self.reset_cycle.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn start_workers(
        &self,
        hwnd: HWND,
        targets: &[String],
        refresh_interval_ms: u64,
        timeout_ms: u64,
        tx: Sender<PingEvent>,
    ) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let hwnd_raw = hwnd as isize;

        for (idx, target) in targets.iter().enumerate() {
            let target = target.clone();
            let tx = tx.clone();
            let running = Arc::clone(&self.running);
            let generation = Arc::clone(&self.generation);
            let reset_cycle = Arc::clone(&self.reset_cycle);
            let method = Arc::clone(&self.method);

            thread::spawn(move || {
                thread::sleep(Duration::from_millis(120));
                let mut dns_cache: HashMap<String, Ipv4Addr> = HashMap::new();
                let mut seq: u64 = (idx as u64).wrapping_mul(1000) + 1;

                while running.load(Ordering::Relaxed) && generation.load(Ordering::Relaxed) == gen {
                    let loop_start = Instant::now();
                    let cycle = reset_cycle.load(Ordering::SeqCst);
                    let active_method = PingMethod::from_u8(method.load(Ordering::Relaxed));

                    let latency = match resolve_ipv4(&target, &mut dns_cache) {
                        Some(ip) => {
                            let res = match active_method {
                                PingMethod::Icmp => ping_icmp(ip, timeout_ms),
                                PingMethod::Tcp => ping_tcp(ip, timeout_ms),
                                PingMethod::Udp => ping_udp(ip, timeout_ms, seq),
                            };
                            if res.is_none() {
                                dns_cache.remove(&target);
                            }
                            res
                        }
                        None => None,
                    };
                    seq = seq.wrapping_add(1);

                    let result = format_latency(latency, timeout_ms);

                    if running.load(Ordering::Relaxed)
                        && generation.load(Ordering::Relaxed) == gen
                        && reset_cycle.load(Ordering::SeqCst) == cycle
                    {
                        if tx
                            .send(PingEvent {
                                cycle,
                                target_idx: idx,
                                result,
                            })
                            .is_ok()
                        {
                            unsafe {
                                let _ = PostMessageW(hwnd_raw as HWND, WM_APP_PING_READY, 0, 0);
                            }
                        }
                    }

                    let elapsed_ms = loop_start.elapsed().as_millis() as u64;
                    if refresh_interval_ms > elapsed_ms {
                        let mut remaining = refresh_interval_ms - elapsed_ms;
                        while remaining > 0
                            && running.load(Ordering::Relaxed)
                            && generation.load(Ordering::Relaxed) == gen
                        {
                            let step = remaining.min(100);
                            thread::sleep(Duration::from_millis(step));
                            remaining -= step;
                        }
                    }
                }
            });
        }
    }
}