//! A minimal D-Bus client: just enough to call methods on the system bus.
//!
//! Replaces spawning `dbus-send` (a new process per LED change, ~200 ms on the
//! rabbit) with one long-lived socket. Supports EXTERNAL auth (the only one the
//! rabbit's dbus-daemon 1.2.24 allows), little-endian method calls with string
//! and int32 arguments, and reading the reply or error. If the connection breaks
//! it reconnects once and retries.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const SYSTEM_BUS: &str = "/var/run/dbus/system_bus_socket";

pub enum Arg<'a> {
    S(&'a str),
    I(i32),
}

struct Conn {
    s: UnixStream,
    serial: u32,
    name: String,
}

static BUS: Mutex<Option<Conn>> = Mutex::new(None);
static PATH: OnceLock<String> = OnceLock::new();

/// Use a different bus socket (tests). Must be called before the first call().
pub fn set_socket(path: &str) {
    let _ = PATH.set(path.to_owned());
}

extern "C" {
    fn getuid() -> u32;
}

// ------------------------------------------------------------------ encoding

fn pad(b: &mut Vec<u8>, align: usize) {
    while b.len() % align != 0 {
        b.push(0);
    }
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    pad(b, 4);
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s.as_bytes());
    b.push(0);
}

fn put_sig(b: &mut Vec<u8>, s: &str) {
    b.push(s.len() as u8);
    b.extend_from_slice(s.as_bytes());
    b.push(0);
}

/// Builds a METHOD_CALL message. Header field codes: 1 PATH, 2 INTERFACE,
/// 3 MEMBER, 6 DESTINATION, 8 SIGNATURE.
pub fn encode_call(serial: u32, dest: &str, path: &str, iface: &str, member: &str, args: &[Arg]) -> Vec<u8> {
    encode(1, serial, Some(dest), path, iface, member, args)
}

/// Builds a broadcast SIGNAL message (no destination).
pub fn encode_signal(serial: u32, path: &str, iface: &str, member: &str, args: &[Arg]) -> Vec<u8> {
    encode(4, serial, None, path, iface, member, args)
}

fn encode(mtype: u8, serial: u32, dest: Option<&str>, path: &str, iface: &str, member: &str, args: &[Arg]) -> Vec<u8> {
    let mut sig = String::new();
    let mut body = Vec::new();
    for a in args {
        match a {
            Arg::S(s) => {
                sig.push('s');
                put_str(&mut body, s);
            }
            Arg::I(i) => {
                sig.push('i');
                pad(&mut body, 4);
                body.extend_from_slice(&i.to_le_bytes());
            }
        }
    }
    let mut m = vec![b'l', mtype, 0, 1];
    m.extend_from_slice(&(body.len() as u32).to_le_bytes());
    m.extend_from_slice(&serial.to_le_bytes());
    m.extend_from_slice(&[0; 4]); // header field array length, filled in below
    let start = m.len();
    let field = |m: &mut Vec<u8>, code: u8, ty: &str, val: &str| {
        pad(m, 8);
        m.push(code);
        put_sig(m, ty);
        if ty == "g" { put_sig(m, val) } else { put_str(m, val) }
    };
    field(&mut m, 1, "o", path);
    field(&mut m, 2, "s", iface);
    field(&mut m, 3, "s", member);
    if let Some(d) = dest {
        field(&mut m, 6, "s", d);
    }
    if !sig.is_empty() {
        field(&mut m, 8, "g", &sig);
    }
    let len = (m.len() - start) as u32;
    m[12..16].copy_from_slice(&len.to_le_bytes());
    pad(&mut m, 8);
    m.extend_from_slice(&body);
    m
}

// ------------------------------------------------------------------ decoding

#[derive(Default)]
pub struct Msg {
    pub mtype: u8, // 1 call, 2 return, 3 error, 4 signal
    pub reply_serial: Option<u32>,
    pub path: String,
    pub iface: String,
    pub member: String,
    pub error: Option<String>,
    pub sender: String,
    pub sig: String,
    pub body: Vec<u8>,
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Parses the header field array into `m`. `f` holds the bytes after the
/// 16-byte fixed header, so absolute offsets (for alignment) are `16 + i`.
fn parse_fields(f: &[u8], len: usize, m: &mut Msg) {
    let al = |i: usize, a: usize| i + ((a - (16 + i) % a) % a);
    let text = |b: Option<&[u8]>| b.map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default();
    let mut i = 0;
    while i < len {
        i = al(i, 8);
        let (Some(&code), Some(&sl)) = (f.get(i), f.get(i + 1)) else { break };
        let sig = f.get(i + 2..i + 2 + sl as usize).unwrap_or(b"");
        i += 3 + sl as usize;
        match sig {
            b"u" => {
                i = al(i, 4);
                if code == 5 {
                    m.reply_serial = u32_at(f, i);
                }
                i += 4;
            }
            b"s" | b"o" => {
                i = al(i, 4);
                let n = u32_at(f, i).unwrap_or(0) as usize;
                let v = text(f.get(i + 4..i + 4 + n));
                match code {
                    1 => m.path = v,
                    2 => m.iface = v,
                    3 => m.member = v,
                    4 => m.error = Some(v),
                    7 => m.sender = v,
                    _ => {}
                }
                i += 5 + n;
            }
            b"g" => {
                let n = *f.get(i).unwrap_or(&0) as usize;
                if code == 8 {
                    m.sig = text(f.get(i + 1..i + 1 + n));
                }
                i += 2 + n;
            }
            _ => break,
        }
    }
}

pub fn read_msg(s: &mut impl Read) -> io::Result<Msg> {
    let mut fixed = [0u8; 16];
    s.read_exact(&mut fixed)?;
    if fixed[0] != b'l' {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "big-endian D-Bus message"));
    }
    let body_len = u32_at(&fixed, 4).unwrap() as usize;
    let fields_len = u32_at(&fixed, 12).unwrap() as usize;
    if body_len > 1 << 20 || fields_len > 1 << 16 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oversized D-Bus message"));
    }
    let mut f = vec![0u8; (fields_len + 7) / 8 * 8];
    s.read_exact(&mut f)?;
    let mut body = vec![0u8; body_len];
    s.read_exact(&mut body)?;
    let mut m = Msg { mtype: fixed[1], body, ..Default::default() };
    parse_fields(&f, fields_len, &mut m);
    Ok(m)
}

/// A decoded body argument (the basic types only; anything else stops decoding).
#[derive(Debug, PartialEq)]
pub enum Value {
    S(String),
    I(i64),
    B(bool),
    Unsupported(char),
}

/// Decodes a body by its signature. Bodies start 8-aligned, so offsets within
/// the body have the same alignment as absolute offsets.
pub fn decode_body(sig: &str, b: &[u8]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let al = |i: usize, a: usize| (i + a - 1) / a * a;
    for c in sig.chars() {
        let v = match c {
            's' | 'o' => {
                i = al(i, 4);
                let Some(n) = u32_at(b, i) else { break };
                let s = b.get(i + 4..i + 4 + n as usize).map(|s| String::from_utf8_lossy(s).into_owned());
                i += 5 + n as usize;
                match s { Some(s) => Value::S(s), None => break }
            }
            'g' => {
                let n = *b.get(i).unwrap_or(&0) as usize;
                let s = b.get(i + 1..i + 1 + n).map(|s| String::from_utf8_lossy(s).into_owned());
                i += 2 + n;
                match s { Some(s) => Value::S(s), None => break }
            }
            'i' | 'u' | 'b' => {
                i = al(i, 4);
                let Some(u) = u32_at(b, i) else { break };
                i += 4;
                match c { 'i' => Value::I(u as i32 as i64), 'u' => Value::I(u as i64), _ => Value::B(u != 0) }
            }
            'y' => {
                let Some(&y) = b.get(i) else { break };
                i += 1;
                Value::I(y as i64)
            }
            'n' | 'q' => {
                i = al(i, 2);
                let Some(x) = b.get(i..i + 2) else { break };
                i += 2;
                let u = u16::from_le_bytes([x[0], x[1]]);
                Value::I(if c == 'n' { u as i16 as i64 } else { u as i64 })
            }
            other => {
                out.push(Value::Unsupported(other));
                break;
            }
        };
        out.push(v);
    }
    out
}

/// The first argument of a body, if it is a string (Hello, GetId, error text).
pub fn first_string(body: &[u8]) -> Option<String> {
    let n = u32_at(body, 0)? as usize;
    body.get(4..4 + n).map(|s| String::from_utf8_lossy(s).into_owned())
}

// ------------------------------------------------------------------ connection

fn read_line(s: &mut UnixStream) -> io::Result<String> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while !line.ends_with(b"\r\n") {
        s.read_exact(&mut b)?;
        line.push(b[0]);
        if line.len() > 512 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "auth line too long"));
        }
    }
    Ok(String::from_utf8_lossy(&line).trim().to_owned())
}

/// Outer error: the socket failed (reconnect). Inner error: D-Bus error reply.
type CallResult = io::Result<Result<Option<String>, String>>;

impl Conn {
    fn open(path: &str) -> io::Result<Conn> {
        let mut s = UnixStream::connect(path)?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?; // a snapshot takes ~1.6 s
        s.set_write_timeout(Some(Duration::from_secs(2)))?;
        let uid: String = unsafe { getuid() }.to_string().bytes().map(|b| format!("{b:02x}")).collect();
        s.write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes())?;
        let ok = read_line(&mut s)?;
        if !ok.starts_with("OK ") {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("D-Bus auth: {ok}")));
        }
        s.write_all(b"BEGIN\r\n")?;
        let mut c = Conn { s, serial: 0, name: String::new() };
        match c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", &[])? {
            Ok(name) => c.name = name.unwrap_or_default(),
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, format!("Hello: {e}"))),
        }
        Ok(c)
    }

    fn call(&mut self, dest: &str, path: &str, iface: &str, member: &str, args: &[Arg]) -> CallResult {
        self.serial = self.serial.wrapping_add(1).max(1);
        let serial = self.serial;
        self.s.write_all(&encode_call(serial, dest, path, iface, member, args))?;
        loop {
            let m = read_msg(&mut self.s)?;
            // Skip signals (e.g. NameAcquired after Hello) and stale replies.
            if m.reply_serial != Some(serial) || !(m.mtype == 2 || m.mtype == 3) {
                continue;
            }
            return Ok(if m.mtype == 2 {
                Ok(first_string(&m.body))
            } else {
                let name = m.error.unwrap_or_else(|| "unknown error".into());
                Err(match first_string(&m.body) {
                    Some(text) => format!("{name}: {text}"),
                    None => name,
                })
            });
        }
    }
}

#[derive(Debug)]
pub enum CallError {
    /// Couldn't reach the bus at all (caller may fall back to dbus-send).
    Transport(String),
    /// The bus or the service answered with an error.
    Remote(String),
}

/// Calls a method on the system bus and waits (up to 5 s) for the reply.
/// Returns the first string of the reply, if it has one.
pub fn call(dest: &str, path: &str, iface: &str, member: &str, args: &[Arg]) -> Result<Option<String>, CallError> {
    let sock = PATH.get().map(String::as_str).unwrap_or(SYSTEM_BUS);
    let mut bus = BUS.lock().unwrap_or_else(|p| p.into_inner());
    let mut last = String::new();
    for _attempt in 0..2 {
        if bus.is_none() {
            match Conn::open(sock) {
                Ok(c) => *bus = Some(c),
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            }
        }
        match bus.as_mut().unwrap().call(dest, path, iface, member, args) {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(remote)) => return Err(CallError::Remote(remote)),
            Err(e) => {
                last = e.to_string();
                *bus = None; // broken socket: reconnect and retry once
            }
        }
    }
    Err(CallError::Transport(last))
}

/// Our unique bus name (":1.42"), connecting first if needed.
pub fn unique_name() -> Result<String, CallError> {
    call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetId", &[])?;
    let bus = BUS.lock().unwrap_or_else(|p| p.into_inner());
    Ok(bus.as_ref().map(|c| c.name.clone()).unwrap_or_default())
}

/// Broadcasts a signal on the shared connection (used only by tests).
pub fn emit(path: &str, iface: &str, member: &str, args: &[Arg]) -> Result<(), CallError> {
    unique_name()?; // make sure we are connected
    let mut bus = BUS.lock().unwrap_or_else(|p| p.into_inner());
    let c = bus.as_mut().ok_or_else(|| CallError::Transport("not connected".into()))?;
    c.serial = c.serial.wrapping_add(1).max(1);
    c.s.write_all(&encode_signal(c.serial, path, iface, member, args))
        .map_err(|e| CallError::Transport(e.to_string()))
}

/// Opens its own connection, subscribes with `rule` (e.g. "type='signal'") and
/// calls `on_signal` for every signal, forever. Reconnects every 3 s if the bus
/// goes away. Meant to run on a dedicated thread.
pub fn listen(rule: &str, mut on_signal: impl FnMut(&Msg)) -> ! {
    let sock = PATH.get().map(String::as_str).unwrap_or(SYSTEM_BUS).to_owned();
    loop {
        let r: io::Result<()> = (|| {
            let mut c = Conn::open(&sock)?;
            let bus = ("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus");
            if let Err(e) = c.call(bus.0, bus.1, bus.2, "AddMatch", &[Arg::S(rule)])? {
                return Err(io::Error::new(io::ErrorKind::Other, format!("AddMatch: {e}")));
            }
            c.s.set_read_timeout(None)?;
            eprintln!("events: listening as {} for {rule}", c.name);
            loop {
                let m = read_msg(&mut c.s)?;
                if m.mtype == 4 {
                    on_signal(&m);
                }
            }
        })();
        if let Err(e) = r {
            eprintln!("events: bus connection lost ({e}); retrying in 3 s");
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}
