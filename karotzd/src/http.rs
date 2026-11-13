//! Minimal HTTP/1.0 request parsing, responses, and a pass-through proxy to the
//! old busybox httpd for anything karotzd hasn't ported yet.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::thread;
use std::time::Duration;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Every byte read from the client so far (head plus any early body bytes).
    pub raw: Vec<u8>,
}

impl Request {
    pub fn q(&self, key: &str) -> &str {
        self.query.get(key).map(String::as_str).unwrap_or("")
    }
}

pub fn read_request(mut s: &TcpStream) -> io::Result<Request> {
    let mut raw = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") && !raw.windows(2).any(|w| w == b"\n\n") {
        let n = s.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if raw.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "header too large"));
        }
    }
    // A POST body (form fields): read it all, then treat its fields like query fields.
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..head_end]).to_ascii_lowercase();
    let body_len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
        .unwrap_or(0)
        .min(64 * 1024);
    while raw.len() < head_end + body_len {
        let n = s.read(&mut buf)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let line_end = raw.iter().position(|&b| b == b'\n').unwrap_or(raw.len());
    let line = String::from_utf8_lossy(&raw[..line_end]).trim().to_owned();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/");
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let mut query = parse_query(qs);
    if body_len > 0 && head.contains("application/x-www-form-urlencoded") {
        let end = (head_end + body_len).min(raw.len());
        for (k, v) in parse_query(&String::from_utf8_lossy(&raw[head_end..end])) {
            query.entry(k).or_insert(v);
        }
    }
    Ok(Request { method, path: path.to_owned(), query, raw })
}

pub fn parse_query(qs: &str) -> HashMap<String, String> {
    qs.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (url_decode(k), url_decode(v))
        })
        .collect()
}

pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c))) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn respond(mut s: &TcpStream, code: u16, ctype: &str, body: &[u8]) -> io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Service Unavailable",
    };
    let head = format!(
        "HTTP/1.0 {code} {reason}\r\nServer: karotzd\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)
}

/// Same Content-Type the old CGI scripts send, so the web UI parses it the same way.
pub fn respond_json(s: &TcpStream, json: &str) -> io::Result<()> {
    let mut body = json.to_owned();
    body.push('\n');
    respond(s, 200, "text/plain", body.as_bytes())
}

/// Forwards the request to the old web server and streams its answer back.
pub fn proxy(client: &TcpStream, req: &Request, upstream: &str) -> io::Result<()> {
    let mut up = match TcpStream::connect(upstream) {
        Ok(u) => u,
        Err(e) => return respond(client, 502, "text/plain", format!("upstream {upstream}: {e}\n").as_bytes()),
    };
    up.set_read_timeout(Some(Duration::from_secs(60)))?;
    up.write_all(&req.raw)?;
    // Client -> upstream (request bodies), in the background.
    let (mut c_in, mut u_out) = (client.try_clone()?, up.try_clone()?);
    let fwd = thread::Builder::new().stack_size(32 * 1024).spawn(move || {
        let _ = io::copy(&mut c_in, &mut u_out);
        let _ = u_out.shutdown(Shutdown::Write);
    })?;
    // Upstream -> client, until the old server closes the connection.
    let mut c_out = client.try_clone()?;
    let r = io::copy(&mut up, &mut c_out);
    let _ = client.shutdown(Shutdown::Both);
    let _ = fwd.join();
    r.map(|_| ())
}

/// A JSON string literal (quotes, backslashes and control characters escaped).
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push(' '),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
