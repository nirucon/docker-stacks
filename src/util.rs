//! Small helpers: local time, opening links, the terminal clipboard.
use std::io::Write;
use std::process::{Command, Stdio};

/// Local (hour, minute, second) and a file-name stamp YYYYmmdd-HHMMSS.
pub fn local(epoch: i64) -> (i32, i32, i32, String) {
    // SAFETY: localtime_r writes only into the struct we pass.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t: libc::time_t = epoch as libc::time_t;
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    );
    (tm.tm_hour, tm.tm_min, tm.tm_sec, stamp)
}

pub fn clock(epoch: i64) -> String {
    let (h, m, _, _) = local(epoch);
    format!("{h:02}:{m:02}")
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Put text on the clipboard of the terminal (OSC 52): works over SSH in
/// most terminals, and through tmux with set-clipboard on.
pub fn osc52(text: &str) {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

pub fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

/// Open a link or a folder with the desktop's opener, detached.
pub fn open(target: &str) -> Result<(), String> {
    if !has_display() {
        return Err("no desktop here".into());
    }
    Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"http://x:80"), "aHR0cDovL3g6ODA=");
    }
}
