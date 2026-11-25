use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE,
};

use crate::ui::win_util::{to_wide, APP_NAME, APP_VERSION, GITHUB_REPO};

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub tag: String,
    pub download_url: String,
}

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    UpToDate,
    Available(UpdateInfo),
    NoReleases,
    Failed,
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// Removes leftover `.old` / `.new` files from a completed or interrupted self-update.
pub fn cleanup_old_binaries() {
    let Ok(exe_path) = std::env::current_exe() else {
        return;
    };

    let new_path = append_suffix(&exe_path, ".new");
    if new_path.exists() {
        let _ = fs::remove_file(&new_path);
    }

    let old_path = append_suffix(&exe_path, ".old");
    if old_path.exists() {
        thread::spawn(move || {
            for _ in 0..40 {
                if fs::remove_file(&old_path).is_ok() || !old_path.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    }
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
    let content = after_colon.strip_prefix('"')?;
    let end_quote = content.find('"')?;
    Some(content[..end_quote].to_string())
}

fn extract_exe_download_url(json: &str, tag: &str) -> String {
    let mut search_from = 0;
    let key = "\"browser_download_url\"";
    while let Some(rel_pos) = json[search_from..].find(key) {
        let abs_pos = search_from + rel_pos + key.len();
        let rest = &json[abs_pos..];
        if let Some(colon) = rest.find(':') {
            let after_colon = rest[colon + 1..].trim_start();
            if let Some(stripped) = after_colon.strip_prefix('"') {
                if let Some(end_q) = stripped.find('"') {
                    let url = &stripped[..end_q];
                    if url.to_ascii_lowercase().ends_with("/nanoping.exe") {
                        return url.to_string();
                    }
                }
            }
        }
        search_from = abs_pos;
    }
    format!("https://github.com/{GITHUB_REPO}/releases/download/{tag}/NanoPing.exe")
}

fn parse_https_url(url: &str) -> Option<(String, String)> {
    let without_scheme = url.strip_prefix("https://")?;
    let (host, path_rest) = without_scheme.split_once('/')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), format!("/{path_rest}")))
}

fn https_get_bytes(
    host_str: &str,
    path_str: &str,
    extra_headers: Option<&str>,
    max_bytes: usize,
    recv_timeout_ms: i32,
) -> Result<(u32, Vec<u8>), String> {
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
            return Err("Failed to initialize network session".to_string());
        }

        let _ = WinHttpSetTimeouts(h_session, 8000, 8000, 10000, recv_timeout_ms);

        let host = to_wide(host_str);
        let h_connect = WinHttpConnect(h_session, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0);
        if h_connect.is_null() {
            WinHttpCloseHandle(h_session);
            return Err("Failed to connect to server".to_string());
        }

        let verb = to_wide("GET");
        let path = to_wide(path_str);
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
            return Err("Failed to create HTTPS request".to_string());
        }

        let hdr_wide = extra_headers.map(to_wide);
        let (hdr_ptr, hdr_len) = match hdr_wide {
            Some(ref w) => (w.as_ptr(), (w.len() - 1) as u32),
            None => (ptr::null(), 0),
        };

        let sent = WinHttpSendRequest(
            h_req,
            hdr_ptr,
            hdr_len,
            ptr::null_mut::<c_void>() as _,
            0,
            0,
            0,
        );

        let mut status_code: u32 = 0;
        let mut body = Vec::new();
        let mut err: Option<String> = None;

        if sent != 0 && WinHttpReceiveResponse(h_req, ptr::null_mut()) != 0 {
            let mut len = std::mem::size_of::<u32>() as u32;
            let _ = WinHttpQueryHeaders(
                h_req,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                &mut status_code as *mut u32 as *mut c_void,
                &mut len,
                ptr::null_mut(),
            );

            loop {
                let mut avail: u32 = 0;
                if WinHttpQueryDataAvailable(h_req, &mut avail) == 0 || avail == 0 {
                    break;
                }
                let to_read = avail.min(32_768);
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
                if body.len() + (read as usize) > max_bytes {
                    err = Some("Downloaded file exceeded size limit".to_string());
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
            }
        } else {
            err = Some("Network request failed".to_string());
        }

        WinHttpCloseHandle(h_req);
        WinHttpCloseHandle(h_connect);
        WinHttpCloseHandle(h_session);

        match err {
            Some(e) => Err(e),
            None => Ok((status_code, body)),
        }
    }
}

pub fn check_for_updates() -> UpdateStatus {
    let path = format!("/repos/{GITHUB_REPO}/releases/latest");
    let Ok((status, bytes)) = https_get_bytes(
        "api.github.com",
        &path,
        Some("Accept: application/vnd.github+json\r\n"),
        256 * 1024,
        8000,
    ) else {
        return UpdateStatus::Failed;
    };

    if status == 404 {
        return UpdateStatus::NoReleases;
    }

    let Ok(json) = String::from_utf8(bytes) else {
        return UpdateStatus::Failed;
    };

    if let Some(tag) = extract_json_string(&json, "tag_name") {
        let current = parse_semver(APP_VERSION).unwrap_or((1, 0, 0));
        if let Some(latest) = parse_semver(&tag) {
            if latest > current {
                let download_url = extract_exe_download_url(&json, &tag);
                return UpdateStatus::Available(UpdateInfo { tag, download_url });
            }
            return UpdateStatus::UpToDate;
        }
    }

    UpdateStatus::Failed
}

/// Downloads the new `NanoPing.exe`, atomically swaps it with the running binary,
/// preserves `config.ini`, and spawns the updated executable.
pub fn download_and_install_update(download_url: &str) -> Result<(), String> {
    let (host, path) =
        parse_https_url(download_url).ok_or_else(|| "Invalid download URL".to_string())?;

    let (status, bytes) = https_get_bytes(&host, &path, None, 20 * 1024 * 1024, 30_000)?;
    if status != 200 {
        return Err(format!("Server returned HTTP {status}"));
    }
    if bytes.len() < 64 * 1024 || bytes.get(0..2) != Some(b"MZ") {
        return Err("Downloaded file is not a valid Windows executable".to_string());
    }

    let exe_path =
        std::env::current_exe().map_err(|_| "Cannot locate current executable".to_string())?;
    let new_path = append_suffix(&exe_path, ".new");
    let old_path = append_suffix(&exe_path, ".old");

    let _ = fs::remove_file(&new_path);
    let _ = fs::remove_file(&old_path);

    // 1. Write new executable beside current executable
    fs::write(&new_path, &bytes)
        .map_err(|_| "Cannot write to folder (check write permissions)".to_string())?;

    // 2. Rename running executable -> .old (allowed natively on Windows)
    if let Err(_) = fs::rename(&exe_path, &old_path) {
        let _ = fs::remove_file(&new_path);
        return Err("Cannot replace running executable".to_string());
    }

    // 3. Move .new -> NanoPing.exe (rollback cleanly on any error)
    if let Err(_) = fs::rename(&new_path, &exe_path) {
        let _ = fs::rename(&old_path, &exe_path);
        let _ = fs::remove_file(&new_path);
        return Err("Failed to finalize update".to_string());
    }

    // 4. Launch the newly installed NanoPing.exe (which will auto-delete .old on startup)
    if let Err(_) = Command::new(&exe_path).spawn() {
        let _ = fs::remove_file(&exe_path);
        let _ = fs::rename(&old_path, &exe_path);
        return Err("Failed to restart NanoPing".to_string());
    }

    Ok(())
}