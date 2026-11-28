use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
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

use crate::config::{PingMethod, TargetEntry, TargetKind};
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

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((input.len() + 2) / 3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn build_socks5_udp_domain(domain: &str, port: u16, payload: &[u8]) -> Vec<u8> {
    let db = domain.as_bytes();
    let mut pkt = Vec::with_capacity(7 + db.len() + payload.len());
    pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x03, db.len() as u8]);
    pkt.extend_from_slice(db);
    pkt.extend_from_slice(&port.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}

fn build_tls_client_hello(sni: &str, seq: u64) -> Vec<u8> {
    let mut exts = Vec::new();
    // Add SNI extension only for non-IP hostnames (RFC 6066)
    if sni.parse::<IpAddr>().is_err() && !sni.is_empty() && sni.len() <= 253 {
        let nb = sni.as_bytes();
        let entry_len = (nb.len() + 3) as u16;
        let ext_len = entry_len + 2;
        exts.extend_from_slice(&[0x00, 0x00]);
        exts.extend_from_slice(&ext_len.to_be_bytes());
        exts.extend_from_slice(&entry_len.to_be_bytes());
        exts.push(0x00);
        exts.extend_from_slice(&(nb.len() as u16).to_be_bytes());
        exts.extend_from_slice(nb);
    }
    // supported_groups: x25519 (0x001d), secp256r1 (0x0017)
    exts.extend_from_slice(&[0x00, 0x0a, 0x00, 0x06, 0x00, 0x04, 0x00, 0x1d, 0x00, 0x17]);

    let mut body = Vec::with_capacity(96 + exts.len());
    body.extend_from_slice(&[0x03, 0x03]); // TLS 1.2
    for i in 0..4u64 {
        body.extend_from_slice(&seq.wrapping_add(i.wrapping_mul(0x9E37_79B9)).to_le_bytes());
    }
    body.push(0x00); // Session ID len
    body.extend_from_slice(&[
        0x00, 0x08, // 4 cipher suites
        0xc0, 0x2f, 0xc0, 0x2b, 0x13, 0x01, 0x00, 0x9c,
    ]);
    body.extend_from_slice(&[0x01, 0x00]); // Compression: null
    body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    body.extend_from_slice(&exts);

    let hs_len = body.len();
    let rec_len = (hs_len + 4) as u16;
    let mut pkt = Vec::with_capacity(hs_len + 9);
    pkt.extend_from_slice(&[0x16, 0x03, 0x01]);
    pkt.extend_from_slice(&rec_len.to_be_bytes());
    pkt.push(0x01); // ClientHello
    pkt.extend_from_slice(&[0x00, ((hs_len >> 8) & 0xFF) as u8, (hs_len & 0xFF) as u8]);
    pkt.extend_from_slice(&body);
    pkt
}

fn parse_proxy_addr(host_port: &str, default_port: u16) -> Option<SocketAddr> {
    let trimmed = host_port.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(sa) = trimmed.parse::<SocketAddr>() {
        return Some(sa);
    }
    if trimmed.contains(':') && !trimmed.starts_with('[') {
        if let Ok(mut it) = trimmed.to_socket_addrs() {
            return it.next();
        }
    }
    (trimmed, default_port).to_socket_addrs().ok()?.next()
}

fn remaining_timeout(deadline: Instant) -> Option<Duration> {
    let now = Instant::now();
    if now >= deadline {
        None
    } else {
        Some(deadline - now)
    }
}

fn socks5_handshake(
    proxy_addr: SocketAddr,
    auth: Option<(&str, &str)>,
    deadline: Instant,
) -> Option<TcpStream> {
    let mut stream = TcpStream::connect_timeout(&proxy_addr, remaining_timeout(deadline)?).ok()?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(remaining_timeout(deadline));
    let _ = stream.set_read_timeout(remaining_timeout(deadline));

    if auth.is_some() {
        stream.write_all(&[0x05, 0x02, 0x00, 0x02]).ok()?;
    } else {
        stream.write_all(&[0x05, 0x01, 0x00]).ok()?;
    }

    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).ok()?;
    if choice[0] != 0x05 {
        return None;
    }

    match choice[1] {
        0x00 => Some(stream),
        0x02 => {
            let (user, pass) = auth?;
            let ub = user.as_bytes();
            let pb = pass.as_bytes();
            if ub.len() > 255 || pb.len() > 255 {
                return None;
            }
            let mut req = Vec::with_capacity(3 + ub.len() + pb.len());
            req.push(0x01);
            req.push(ub.len() as u8);
            req.extend_from_slice(ub);
            req.push(pb.len() as u8);
            req.extend_from_slice(pb);
            stream.write_all(&req).ok()?;

            let mut resp = [0u8; 2];
            stream.read_exact(&mut resp).ok()?;
            if resp[1] == 0x00 {
                Some(stream)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn socks5_read_bound_addr(stream: &mut TcpStream) -> Option<(IpAddr, u16)> {
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).ok()?;
    if hdr[0] != 0x05 || hdr[1] != 0x00 {
        return None;
    }
    let ip = match hdr[3] {
        0x01 => {
            let mut b = [0u8; 4];
            stream.read_exact(&mut b).ok()?;
            IpAddr::V4(Ipv4Addr::from(b))
        }
        0x04 => {
            let mut b = [0u8; 16];
            stream.read_exact(&mut b).ok()?;
            IpAddr::V6(Ipv6Addr::from(b))
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).ok()?;
            let mut dom = vec![0u8; len[0] as usize];
            stream.read_exact(&mut dom).ok()?;
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        }
        _ => return None,
    };
    let mut p = [0u8; 2];
    stream.read_exact(&mut p).ok()?;
    Some((ip, u16::from_be_bytes(p)))
}

fn tls_probe_rtt(stream: &mut TcpStream, probe_host: &str, seq: u64, deadline: Instant) -> Option<u32> {
    let rem = remaining_timeout(deadline)?;
    let _ = stream.set_write_timeout(Some(rem));
    let _ = stream.set_read_timeout(Some(rem));

    let hello = build_tls_client_hello(probe_host, seq);
    let start = Instant::now();
    stream.write_all(&hello).ok()?;

    let mut hdr = [0u8; 5];
    stream.read_exact(&mut hdr).ok()?;
    if hdr[0] == 0x16 || hdr[0] == 0x15 {
        Some(start.elapsed().as_millis() as u32)
    } else {
        None
    }
}

fn ping_socks5_tcp(entry: &TargetEntry, probe_host: &str, timeout_ms: u64, seq: u64) -> Option<u32> {
    let (auth, host_port) = entry.split_proxy_auth();
    let proxy_addr = parse_proxy_addr(host_port, 1080)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let mut stream = socks5_handshake(proxy_addr, auth, deadline)?;

    let mut req = Vec::with_capacity(10 + probe_host.len());
    req.extend_from_slice(&[0x05, 0x01, 0x00]); // CONNECT
    if let Ok(v4) = probe_host.parse::<Ipv4Addr>() {
        req.push(0x01);
        req.extend_from_slice(&v4.octets());
    } else {
        let hb = probe_host.as_bytes();
        if hb.is_empty() || hb.len() > 255 {
            return None;
        }
        req.push(0x03);
        req.push(hb.len() as u8);
        req.extend_from_slice(hb);
    }
    req.extend_from_slice(&443u16.to_be_bytes());

    let _ = stream.set_write_timeout(remaining_timeout(deadline));
    let _ = stream.set_read_timeout(remaining_timeout(deadline));
    stream.write_all(&req).ok()?;
    let _ = socks5_read_bound_addr(&mut stream)?;

    tls_probe_rtt(&mut stream, probe_host, seq, deadline)
}

/// Real end-to-end SOCKS5 UDP pass-through test using RFC 5389 STUN Binding Requests
/// (immune to sing-box/Xray local DNS port 53 hijacking and QUIC port 443 blocking).
fn ping_socks5_udp(entry: &TargetEntry, timeout_ms: u64, seq: u64) -> Option<u32> {
    let (auth, host_port) = entry.split_proxy_auth();
    let proxy_addr = parse_proxy_addr(host_port, 1080)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    // Keep `_ctrl` alive until the UDP exchange completes (RFC 1928 §7)
    let mut _ctrl = socks5_handshake(proxy_addr, auth, deadline)?;
    let _ = _ctrl.set_write_timeout(remaining_timeout(deadline));
    let _ = _ctrl.set_read_timeout(remaining_timeout(deadline));
    _ctrl
        .write_all(&[0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .ok()?;

    let (bnd_ip, bnd_port) = socks5_read_bound_addr(&mut _ctrl)?;
    if bnd_port == 0 {
        return None;
    }
    let relay_ip = if bnd_ip.is_unspecified() {
        proxy_addr.ip()
    } else {
        bnd_ip
    };
    let relay_addr = SocketAddr::new(relay_ip, bnd_port);

    let bind_addr = if relay_addr.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let udp = UdpSocket::bind(bind_addr).ok()?;

    // Build RFC 5389 STUN Binding Request with a unique 12-byte Transaction ID
    let mut tx_id = [0u8; 12];
    tx_id[0..8].copy_from_slice(&seq.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
    tx_id[8..12].copy_from_slice(&((seq ^ 0xA5A5_5A5A) as u32).to_le_bytes());

    let mut stun_req = [0u8; 20];
    stun_req[0] = 0x00;
    stun_req[1] = 0x01; // Binding Request
    stun_req[4..8].copy_from_slice(&[0x21, 0x12, 0xA4, 0x42]); // Magic Cookie
    stun_req[8..20].copy_from_slice(&tx_id);

    let pkt_cf = build_socks5_udp_domain("stun.cloudflare.com", 3478, &stun_req);
    let pkt_gg = build_socks5_udp_domain("stun.l.google.com", 19302, &stun_req);

    let start = Instant::now();
    let _ = udp.send_to(&pkt_cf, relay_addr);
    let _ = udp.send_to(&pkt_gg, relay_addr);

    let mut buf = [0u8; 1500];
    loop {
        let rem = remaining_timeout(deadline)?;
        let _ = udp.set_read_timeout(Some(rem));
        let (n, _) = udp.recv_from(&mut buf).ok()?;
        if n < 10 || buf[0] != 0x00 || buf[1] != 0x00 {
            continue;
        }
        let hdr_len = match buf[3] {
            0x01 => 10,
            0x04 => 22,
            0x03 => 7 + (buf[4] as usize),
            _ => continue,
        };
        if n >= hdr_len + 20 {
            let p = &buf[hdr_len..n];
            if p[0] == 0x01
                && p[1] == 0x01
                && p[4..8] == [0x21, 0x12, 0xA4, 0x42]
                && p[8..20] == tx_id
            {
                return Some(start.elapsed().as_millis() as u32);
            }
        }
    }
}

fn ping_http_tcp(entry: &TargetEntry, probe_host: &str, timeout_ms: u64, seq: u64) -> Option<u32> {
    let (auth, host_port) = entry.split_proxy_auth();
    let proxy_addr = parse_proxy_addr(host_port, 8080)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let mut stream = TcpStream::connect_timeout(&proxy_addr, remaining_timeout(deadline)?).ok()?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(remaining_timeout(deadline));
    let _ = stream.set_read_timeout(remaining_timeout(deadline));

    let mut req = format!("CONNECT {probe_host}:443 HTTP/1.1\r\nHost: {probe_host}:443\r\n");
    if let Some((user, pass)) = auth {
        let token = base64_encode(format!("{user}:{pass}").as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).ok()?;

    let mut resp = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while resp.len() < 4096 {
        let _ = stream.set_read_timeout(remaining_timeout(deadline));
        stream.read_exact(&mut byte).ok()?;
        resp.push(byte[0]);
        if resp.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let first_line = String::from_utf8_lossy(&resp);
    let status_line = first_line.lines().next()?;
    let status_code = status_line.split_whitespace().nth(1)?;
    if status_code != "200" {
        return None;
    }

    tls_probe_rtt(&mut stream, probe_host, seq, deadline)
}

fn encode_override(m: Option<PingMethod>) -> u8 {
    match m {
        None => 0,
        Some(PingMethod::Icmp) => 1,
        Some(PingMethod::Tcp) => 2,
        Some(PingMethod::Udp) => 3,
    }
}

fn decode_override(v: u8) -> Option<PingMethod> {
    match v {
        1 => Some(PingMethod::Icmp),
        2 => Some(PingMethod::Tcp),
        3 => Some(PingMethod::Udp),
        _ => None,
    }
}

pub struct PingEngine {
    running: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    method: Arc<AtomicU8>,
    target_cycles: Vec<Arc<AtomicU64>>,
    target_overrides: Vec<Arc<AtomicU8>>,
}

impl PingEngine {
    pub fn new(initial_method: PingMethod) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            generation: Arc::new(AtomicU64::new(0)),
            method: Arc::new(AtomicU8::new(initial_method as u8)),
            target_cycles: Vec::new(),
            target_overrides: Vec::new(),
        }
    }

    pub fn set_method(&self, method: PingMethod) {
        self.method.store(method as u8, Ordering::Relaxed);
    }

    pub fn set_target_method(&self, idx: usize, method: Option<PingMethod>) {
        if let Some(ov) = self.target_overrides.get(idx) {
            ov.store(encode_override(method), Ordering::Relaxed);
        }
    }

    pub fn bump_target_cycle(&self, idx: usize) {
        if let Some(c) = self.target_cycles.get(idx) {
            c.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn bump_all_cycles(&self) {
        for c in &self.target_cycles {
            c.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn target_cycle(&self, idx: usize) -> u64 {
        self.target_cycles
            .get(idx)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn start_workers(
        &mut self,
        hwnd: HWND,
        targets: &[TargetEntry],
        proxy_probe_host: &str,
        refresh_interval_ms: u64,
        timeout_ms: u64,
        tx: Sender<PingEvent>,
    ) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let hwnd_raw = hwnd as isize;
        let probe_host_shared = proxy_probe_host.to_string();

        self.target_cycles.clear();
        self.target_overrides.clear();

        for (idx, target) in targets.iter().enumerate() {
            let my_cycle = Arc::new(AtomicU64::new(0));
            let my_override = Arc::new(AtomicU8::new(encode_override(target.method)));
            self.target_cycles.push(Arc::clone(&my_cycle));
            self.target_overrides.push(Arc::clone(&my_override));

            let mut target = target.clone();
            let probe_host = probe_host_shared.clone();
            let tx = tx.clone();
            let running = Arc::clone(&self.running);
            let generation = Arc::clone(&self.generation);
            let method = Arc::clone(&self.method);

            thread::spawn(move || {
                thread::sleep(Duration::from_millis(120));
                let mut dns_cache: HashMap<String, Ipv4Addr> = HashMap::new();
                let mut seq: u64 = (idx as u64).wrapping_mul(1000) + 1;

                while running.load(Ordering::Relaxed) && generation.load(Ordering::Relaxed) == gen {
                    let loop_start = Instant::now();
                    let cycle = my_cycle.load(Ordering::SeqCst);
                    target.method = decode_override(my_override.load(Ordering::Relaxed));
                    let global_method = PingMethod::from_u8(method.load(Ordering::Relaxed));
                    let active_method = target.effective_method(global_method);

                    let latency = match target.kind {
                        TargetKind::Direct => match resolve_ipv4(&target.address, &mut dns_cache) {
                            Some(ip) => {
                                let res = match active_method {
                                    PingMethod::Icmp => ping_icmp(ip, timeout_ms),
                                    PingMethod::Tcp => ping_tcp(ip, timeout_ms),
                                    PingMethod::Udp => ping_udp(ip, timeout_ms, seq),
                                };
                                if res.is_none() {
                                    dns_cache.remove(&target.address);
                                }
                                res
                            }
                            None => None,
                        },
                        TargetKind::Socks5 => match active_method {
                            PingMethod::Udp => ping_socks5_udp(&target, timeout_ms, seq),
                            _ => ping_socks5_tcp(&target, &probe_host, timeout_ms, seq),
                        },
                        TargetKind::Http => ping_http_tcp(&target, &probe_host, timeout_ms, seq),
                    };
                    seq = seq.wrapping_add(1);

                    let result = format_latency(latency, timeout_ms);

                    if running.load(Ordering::Relaxed)
                        && generation.load(Ordering::Relaxed) == gen
                        && my_cycle.load(Ordering::SeqCst) == cycle
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
                            && my_cycle.load(Ordering::SeqCst) == cycle
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