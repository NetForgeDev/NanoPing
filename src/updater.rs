use std::ffi::c_void;
use std::ptr;

use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE,
};

use crate::ui::win_util::{to_wide, APP_NAME, APP_VERSION, GITHUB_REPO};

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    UpToDate,
    Available(String),
    NoReleases,
    Failed,
}

fn parse_semver(tag: &str) -> Option<(u32, u32, u32)> {
    let cleaned = tag.trim().trim_start_matches(['v', 'V']);
    let core = cleaned.split('-').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((major, minor, patch))
}

fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let key_pos = json.find(&pattern)?;
    let rest = &json[key_pos + pattern.len()..];
    let colon_pos = rest.find(':')?;
    let after_colon = rest[colon_pos + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let content = &after_colon[1..];
    let end_quote = content.find('"')?;
    Some(content[..end_quote].to_string())
}

fn https_get_github_latest() -> Option<String> {
    unsafe {
        let agent = to_wide(&format!("{APP_NAME}/{APP_VERSION}"));
        let mut h_session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        );
        if h_session.is_null() {
            h_session = WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                ptr::null(),
                ptr::null(),
                0,
            );
        }
        if h_session.is_null() {
            return None;
        }

        let _ = WinHttpSetTimeouts(h_session, 5000, 5000, 5000, 5000);

        let host = to_wide("api.github.com");
        let h_connect = WinHttpConnect(h_session, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0);
        if h_connect.is_null() {
            WinHttpCloseHandle(h_session);
            return None;
        }

        let verb = to_wide("GET");
        let path = to_wide(&format!("/repos/{GITHUB_REPO}/releases/latest"));
        let h_req = WinHttpOpenRequest(
            h_connect,
            verb.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null::<*const u16>() as _,
            WINHTTP_FLAG_SECURE,
        );
        if h_req.is_null() {
            WinHttpCloseHandle(h_connect);
            WinHttpCloseHandle(h_session);
            return None;
        }

        let headers = to_wide("Accept: application/vnd.github+json\r\n");
        let sent = WinHttpSendRequest(
            h_req,
            headers.as_ptr(),
            (headers.len() - 1) as u32,
            ptr::null_mut::<c_void>() as _,
            0,
            0,
            0,
        );

        let mut body = Vec::new();
        let ok = sent != 0 && WinHttpReceiveResponse(h_req, ptr::null_mut()) != 0;

        if ok {
            loop {
                let mut avail: u32 = 0;
                if WinHttpQueryDataAvailable(h_req, &mut avail) == 0 || avail == 0 {
                    break;
                }
                let to_read = avail.min(16384);
                let mut chunk = vec![0u8; to_read as usize];
                let mut read: u32 = 0;
                if WinHttpReadData(
                    h_req,
                    chunk.as_mut_ptr() as *mut c_void,
                    to_read,
                    &mut read,
                ) == 0
                    || read == 0
                {
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
                if body.len() > 131_072 {
                    break;
                }
            }
        }

        WinHttpCloseHandle(h_req);
        WinHttpCloseHandle(h_connect);
        WinHttpCloseHandle(h_session);

        if ok && !body.is_empty() {
            String::from_utf8(body).ok()
        } else {
            None
        }
    }
}

pub fn check_for_updates() -> UpdateStatus {
    let Some(json) = https_get_github_latest() else {
        return UpdateStatus::Failed;
    };

    if let Some(tag) = extract_json_string(&json, "tag_name") {
        let current = parse_semver(APP_VERSION).unwrap_or((1, 0, 0));
        if let Some(latest) = parse_semver(&tag) {
            if latest > current {
                return UpdateStatus::Available(tag);
            }
            return UpdateStatus::UpToDate;
        }
    }

    if let Some(msg) = extract_json_string(&json, "message") {
        if msg.eq_ignore_ascii_case("Not Found") {
            return UpdateStatus::NoReleases;
        }
    }

    UpdateStatus::Failed
}