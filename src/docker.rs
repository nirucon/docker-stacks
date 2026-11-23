//! The Docker Engine API over its Unix socket.
//!
//! A few hundred lines of HTTP/1.1 instead of an HTTP stack: requests are
//! small, the daemon is local, and streams (events, stats) are
//! newline-delimited JSON in chunked encoding.
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Docker {
    pub socket: PathBuf,
}

/// Where the daemon listens: `DOCKER_HOST=unix://…`, the system socket, or a
/// rootless one.
pub fn find_socket() -> Option<PathBuf> {
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if let Some(p) = host.strip_prefix("unix://") {
            return Some(PathBuf::from(p));
        }
        if !host.is_empty() {
            return None;
        }
    }
    let mut candidates = vec![PathBuf::from("/var/run/docker.sock")];
    if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(Path::new(&run).join("docker.sock"));
    }
    candidates.into_iter().find(|p| p.exists())
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|e| format!("bad answer from docker: {e}"))
    }

    /// The daemon's `message`, or the status.
    pub fn error(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("docker answered {}", self.status))
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn read_head(r: &mut impl BufRead) -> Result<(u16, bool, Option<usize>), String> {
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no answer from docker")?;
    let mut chunked = false;
    let mut length = None;
    loop {
        line.clear();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            if k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse().ok();
            }
        }
    }
    Ok((status, chunked, length))
}

/// One chunk of a chunked body, or None at its end.
fn read_chunk(r: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut line = String::new();
    if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16)
        .map_err(|_| "bad chunk from docker".to_string())?;
    if size == 0 {
        return Ok(None);
    }
    let mut buf = vec![0; size];
    r.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let mut crlf = [0u8; 2];
    let _ = r.read_exact(&mut crlf);
    Ok(Some(buf))
}

pub fn decode_body(
    r: &mut impl BufRead,
    chunked: bool,
    length: Option<usize>,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    if chunked {
        while let Some(c) = read_chunk(r)? {
            body.extend_from_slice(&c);
        }
    } else if let Some(n) = length {
        body.resize(n, 0);
        r.read_exact(&mut body).map_err(|e| e.to_string())?;
    } else {
        r.read_to_end(&mut body).map_err(|e| e.to_string())?;
    }
    Ok(body)
}

impl Docker {
    pub fn new() -> Result<Self, String> {
        let socket = find_socket().ok_or_else(|| {
            "no Docker socket: is Docker installed and running? (DOCKER_HOST must be unix://)"
                .to_string()
        })?;
        Ok(Self { socket })
    }

    fn connect(&self, timeout: Option<Duration>) -> Result<UnixStream, String> {
        let s = UnixStream::connect(&self.socket).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                format!(
                    "no access to {}: add yourself to the docker group (sudo usermod -aG docker $USER) and log in again",
                    self.socket.display()
                )
            } else {
                format!("cannot reach Docker at {}: {e}", self.socket.display())
            }
        })?;
        s.set_read_timeout(timeout).map_err(|e| e.to_string())?;
        Ok(s)
    }

    fn send(
        &self,
        s: &mut UnixStream,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<(), String> {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: docker\r\nUser-Agent: docker-stacks\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes())
            .and_then(|_| s.write_all(body.as_bytes()))
            .map_err(|e| e.to_string())
    }

    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        timeout: u64,
    ) -> Result<Response, String> {
        let mut s = self.connect(Some(Duration::from_secs(timeout)))?;
        self.send(&mut s, method, path, body)?;
        let mut r = BufReader::new(s);
        let (status, chunked, length) = read_head(&mut r)?;
        let body = decode_body(&mut r, chunked, length)?;
        Ok(Response { status, body })
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        let r = self.request("GET", path, None, 30)?;
        if !r.ok() {
            return Err(r.error());
        }
        r.json()
    }

    /// POST or DELETE that answers with nothing much: Ok or the error.
    pub fn act(&self, method: &str, path: &str, timeout: u64) -> Result<Value, String> {
        let r = self.request(method, path, None, timeout)?;
        if !r.ok() {
            return Err(r.error());
        }
        Ok(r.json().unwrap_or(Value::Null))
    }

    /// A stream of JSON lines; `each` returns false to stop.
    pub fn stream(&self, path: &str, mut each: impl FnMut(Value) -> bool) -> Result<(), String> {
        let mut s = self.connect(None)?;
        self.send(&mut s, "GET", path, None)?;
        let mut r = BufReader::new(s);
        let (status, chunked, length) = read_head(&mut r)?;
        if !(200..300).contains(&status) {
            let body = decode_body(&mut r, chunked, length)?;
            return Err(Response { status, body }.error());
        }
        let mut pending = Vec::new();
        let mut handle = |pending: &mut Vec<u8>| -> bool {
            while let Some(i) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=i).collect();
                if let Ok(v) = serde_json::from_slice::<Value>(&line) {
                    if !each(v) {
                        return false;
                    }
                }
            }
            true
        };
        if chunked {
            while let Some(c) = read_chunk(&mut r)? {
                pending.extend_from_slice(&c);
                if !handle(&mut pending) {
                    return Ok(());
                }
            }
        } else {
            let mut buf = [0u8; 8192];
            loop {
                let n = r.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                pending.extend_from_slice(&buf[..n]);
                if !handle(&mut pending) {
                    return Ok(());
                }
            }
        }
        pending.push(b'\n');
        handle(&mut pending);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunked_body_is_joined() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n2\r\n1}\r\n0\r\n\r\n";
        let mut r = BufReader::new(&raw[..]);
        let (status, chunked, length) = read_head(&mut r).unwrap();
        assert_eq!((status, chunked, length), (200, true, None));
        let body = decode_body(&mut r, chunked, length).unwrap();
        assert_eq!(body, b"{\"a\":1}");
    }

    #[test]
    fn a_sized_body_is_read() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Length: 18\r\n\r\n{\"message\":\"gone\"}";
        let mut r = BufReader::new(&raw[..]);
        let (status, chunked, length) = read_head(&mut r).unwrap();
        let body = decode_body(&mut r, chunked, length).unwrap();
        let resp = Response { status, body };
        assert!(!resp.ok());
        assert_eq!(resp.error(), "gone");
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(
            enc(r#"{"dangling":["true"]}"#),
            "%7B%22dangling%22%3A%5B%22true%22%5D%7D"
        );
    }
}
