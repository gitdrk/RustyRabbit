//! Rabbit events (RFID tags, button presses, any com.mindscape.karotz signal)
//! collected from the system bus and served over HTTP.
//!
//! karotzd only observes: the existing /usr/scripts/dbus_watcher still runs the
//! actions (sounds, tag actions, triple-click reboot), so nothing fires twice.
//!
//!   GET /api/events?since=N&wait=S   JSON array of events with seq > N. If there
//!                                    are none yet, waits up to S seconds (max 60).
//!   GET /api/events/stream           Server-Sent Events: one `data:` line per event.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::dbus::{self, Msg, Value};
use crate::http::{json_str, respond, Request};

const KEEP: usize = 200;

struct Store {
    next_seq: u64,
    events: VecDeque<(u64, String)>, // (seq, JSON)
}

static STORE: Mutex<Store> = Mutex::new(Store { next_seq: 1, events: VecDeque::new() });
static NEW_EVENT: Condvar = Condvar::new();

fn uptime_ms() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map(|f| (f * 1000.0) as u64)
        .unwrap_or(0)
}

/// Signals worth keeping: the rabbit's own services, plus karotzd's test interface.
pub fn interesting(m: &Msg) -> bool {
    m.path.starts_with("/com/mindscape/") || m.iface.starts_with("com.mindscape.") || m.iface == "com.karotzd.Test"
}

/// Turns a signal into one JSON object, adding friendly fields for known ones.
pub fn to_json(seq: u64, at_ms: u64, m: &Msg) -> String {
    let vals = dbus::decode_body(&m.sig, &m.body);
    let args: Vec<String> = vals
        .iter()
        .map(|v| match v {
            Value::S(s) => json_str(s),
            Value::I(i) => i.to_string(),
            Value::B(b) => b.to_string(),
            Value::Unsupported(c) => json_str(&format!("<unsupported type {c}>")),
        })
        .collect();
    let has = |s: &str| m.path.contains(s) || m.iface.contains(s);
    // e.g. the LED daemon announces event(<request uuid>, 4) after each command.
    let source = [("Rfid", "rfid"), ("Button", "button"), ("Led", "led"), ("Ears", "ears"), ("Webcam", "webcam")]
        .iter()
        .find(|(k, _)| has(k))
        .map(|(_, v)| *v)
        .unwrap_or(if m.iface == "com.karotzd.Test" { "test" } else { "other" });
    let mut extra = String::new();
    if source == "rfid" && m.member == "tag_in" {
        // tag_in(id, flag, app, type, picture, color), as read by dbus_watcher
        let s = |i: usize| match vals.get(i) { Some(Value::S(s)) => json_str(s), _ => "null".into() };
        let n = |i: usize| match vals.get(i) { Some(Value::I(n)) => n.to_string(), _ => "null".into() };
        extra = format!(",\"tag\":{},\"tag_type\":{},\"tag_color\":{}", s(0), n(3), n(5));
    } else if source == "rfid" && m.member == "tag_out" {
        if let Some(Value::S(id)) = vals.first() {
            extra = format!(",\"tag\":{}", json_str(id));
        }
    }
    format!(
        "{{\"seq\":{seq},\"uptime_ms\":{at_ms},\"source\":\"{source}\",\"event\":{}{extra},\"path\":{},\"interface\":{},\"args\":[{}]}}",
        json_str(&m.member), json_str(&m.path), json_str(&m.iface), args.join(",")
    )
}

pub fn record(m: &Msg) {
    let at = uptime_ms();
    let mut st = STORE.lock().unwrap_or_else(|p| p.into_inner());
    let seq = st.next_seq;
    st.next_seq += 1;
    let json = to_json(seq, at, m);
    eprintln!("event {json}");
    st.events.push_back((seq, json));
    while st.events.len() > KEEP {
        st.events.pop_front();
    }
    NEW_EVENT.notify_all();
}

/// Starts the bus listener thread.
pub fn start() {
    let _ = thread::Builder::new().name("events".into()).stack_size(128 * 1024).spawn(|| {
        dbus::listen("type='signal'", |m| {
            if interesting(m) {
                record(m);
            }
        })
    });
}

/// Events with seq > since, waiting up to `wait` for the first one.
fn since(since: u64, wait: Duration) -> Vec<(u64, String)> {
    let mut st = STORE.lock().unwrap_or_else(|p| p.into_inner());
    if wait > Duration::ZERO && st.next_seq <= since + 1 {
        st = NEW_EVENT.wait_timeout_while(st, wait, |s| s.next_seq <= since + 1).unwrap_or_else(|p| p.into_inner()).0;
    }
    st.events.iter().filter(|(s, _)| *s > since).cloned().collect()
}

/// GET /api/events?since=N&wait=S
pub fn poll(stream: &TcpStream, req: &Request) -> io::Result<()> {
    let n = req.q("since").parse().unwrap_or(0);
    let wait = req.q("wait").parse::<u64>().unwrap_or(0).min(60);
    let evs = since(n, Duration::from_secs(wait));
    let body: Vec<&str> = evs.iter().map(|(_, j)| j.as_str()).collect();
    respond(stream, 200, "application/json", format!("[{}]\n", body.join(",")).as_bytes())
}

/// GET /api/events/stream  (Server-Sent Events; a comment every 15 s keeps it alive)
pub fn stream(mut s: &TcpStream, req: &Request) -> io::Result<()> {
    s.write_all(b"HTTP/1.0 200 OK\r\nServer: karotzd\r\nContent-Type: text/event-stream\r\n\
                  Cache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\n\r\n")?;
    // Start after whatever already happened, unless ?since=N asks for history.
    let mut last = req.q("since").parse().unwrap_or_else(|_| STORE.lock().map(|st| st.next_seq - 1).unwrap_or(0));
    s.write_all(b": karotzd events\n\n")?;
    loop {
        let evs = since(last, Duration::from_secs(15));
        if evs.is_empty() {
            s.write_all(b": keep-alive\n\n")?;
        }
        for (seq, json) in evs {
            write!(s, "id: {seq}\ndata: {json}\n\n")?;
            last = seq;
        }
    }
}
