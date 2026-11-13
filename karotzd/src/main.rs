//! karotzd: a compiled web server for the Karotz rabbit.
//!
//! Modes:
//!   karotzd serve [port] [--readonly] [--upstream host:port] [--datadir dir]
//!       HTTP server (default port 8080). Native routes: /hello, /api/info,
//!       /cgi-bin/leds, /cgi-bin/ears, /cgi-bin/ears_reset, /cgi-bin/sound.
//!       Everything else is passed through to the old web server (127.0.0.1:80).
//!       --readonly never writes the LED memory files in <datadir>/Run.
//!       Also serves rabbit events (RFID, button) at /api/events and
//!       /api/events/stream; --no-events turns the bus listener off.
//!   karotzd listen [socket]
//!       Prints rabbit events (RFID tags, button presses) as they happen.
//!   karotzd selftest
//!       Exercises threads, atomics, sockets, processes, files and the request
//!       parsing, i.e. everything Linux 2.6.28 on ARMv4T might trip on. It never
//!       touches the hardware: routes run against an empty temporary data dir.
//!   karotzd child
//!       Used by selftest to check process spawning.
//!
//! When run as PID 1 (inside the emulator's initramfs) it mounts /proc, brings up
//! loopback, runs the self-test and powers the VM off. It never does that on a
//! real system, where it is never PID 1.

mod dbus;
mod events;
mod http;
mod karotz;
mod routes;
mod util;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use std::{env, fs, process, thread};

static UPSTREAM: OnceLock<String> = OnceLock::new();

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if env::var_os("PATH").is_none() {
        // Started without a login shell (e.g. from inetd or init): use the rabbit's PATH.
        env::set_var("PATH", "/bin:/sbin:/usr/bin:/usr/sbin:/karotz/bin");
    }
    let pid1 = process::id() == 1;
    if pid1 {
        sys::pid1_setup();
    }
    let mode = args.get(1).map(String::as_str).unwrap_or(if pid1 { "selftest" } else { "serve" });
    let code = match mode {
        "serve" => {
            let port = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(8080u16);
            let readonly = args.iter().any(|a| a == "--readonly");
            let data = flag_value(&args, "--datadir").unwrap_or_else(|| "/usr/openkarotz".into());
            let _ = UPSTREAM.set(flag_value(&args, "--upstream").unwrap_or_else(|| "127.0.0.1:80".into()));
            karotz::init(karotz::Ctx { data: data.into(), readonly });
            if let Some(bus) = flag_value(&args, "--bus") {
                dbus::set_socket(&bus);
            }
            if !args.iter().any(|a| a == "--no-events") {
                events::start();
            }
            serve(port, readonly)
        }
        "selftest" => selftest(),
        "dbus-test" => dbus_test(&args),
        "listen" => {
            if let Some(sock) = args.get(2) {
                dbus::set_socket(sock);
            }
            println!("listening for rabbit events (Ctrl+C to stop)");
            dbus::listen("type='signal'", |m| {
                if events::interesting(m) {
                    println!("{}", events::to_json(0, 0, m));
                }
            })
        }
        "child" => {
            println!("child-ok");
            0
        }
        _ => {
            eprintln!("usage: karotzd [serve [port] [--readonly] [--upstream h:p] [--datadir d] | selftest]");
            2
        }
    };
    if pid1 {
        sys::power_off();
    }
    process::exit(code);
}

// ---------------------------------------------------------------- HTTP

fn serve(port: u16, readonly: bool) -> i32 {
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bind :{port}: {e}");
            return 1;
        }
    };
    println!("karotzd listening on :{port}{}", if readonly { " (readonly)" } else { "" });
    let active = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        // Bounded thread-per-connection: plenty for one rabbit, small stacks.
        if active.load(Ordering::Relaxed) >= 16 {
            let _ = http::respond(&stream, 503, "text/plain", b"busy\n");
            continue;
        }
        active.fetch_add(1, Ordering::Relaxed);
        let a = active.clone();
        let spawned = thread::Builder::new().stack_size(128 * 1024).spawn(move || {
            let _ = handle(stream);
            a.fetch_sub(1, Ordering::Relaxed);
        });
        if spawned.is_err() {
            active.fetch_sub(1, Ordering::Relaxed);
        }
    }
    0
}

/// The new web UI: one self-contained page, built into the binary.
const UI_HTML: &str = include_str!("ui.html");

type JsonRoute = fn(&http::Request) -> String;
type RawRoute = fn(&http::Request, &TcpStream) -> std::io::Result<()>;

/// Old CGI paths answered natively with JSON (same replies as the bash scripts).
fn json_route(path: &str) -> Option<JsonRoute> {
    let p = path.strip_prefix("/cgi-bin/")?;
    Some(match p {
        "leds" => karotz::leds,
        "ears" => karotz::ears,
        "ears_reset" => karotz::ears_reset,
        "sound" => karotz::sound,
        "status" => routes::status,
        "get_version" => routes::get_version,
        "get_free_space" => routes::get_free_space,
        "sleep" => routes::sleep,
        "wakeup" => routes::wakeup,
        "ears_random" => routes::ears_random,
        "ears_mode" => routes::ears_mode,
        "reboot" => routes::reboot,
        "setclock" => routes::setclock,
        "health_check_config" => routes::health_check_config,
        "check_health" => routes::check_health,
        "sound_control" => routes::sound_control,
        "stories" => routes::stories,
        "apps/clock" => routes::apps_clock,
        "apps/moods" => routes::apps_moods,
        "moods" => routes::moods_obsolete,
        "sound_list" => routes::sound_list,
        "voice_list" => routes::voice_list,
        "apps_list" => routes::apps_list,
        "moods_list" => routes::moods_list,
        "radios_list" => routes::radios_list,
        "stories_list" => routes::stories_list,
        "display_cache" => routes::display_cache,
        "clear_cache" => routes::clear_cache,
        "snapshot" => routes::snapshot,
        "snapshot_list" => routes::snapshot_list,
        "clear_snapshots" => routes::clear_snapshots,
        "tts" => routes::tts,
        "rfid_list" => routes::rfid_list,
        "rfid_list_ext" => routes::rfid_list_ext,
        "rfid_info" => routes::rfid_info,
        "rfid_rename" => routes::rfid_rename,
        "rfid_delete" => routes::rfid_delete,
        "rfid_unassign" => routes::rfid_unassign,
        "rfid_start_record" => routes::rfid_start_record,
        "rfid_stop_record" => routes::rfid_stop_record,
        "rfid_assign_url" => routes::rfid_assign_url,
        "rfid_assign_karotz" => routes::rfid_assign_karotz,
        "rfid_test_url" => routes::rfid_test_url,
        "clear_rfid" => routes::clear_rfid,
        "tag_list" => routes::tag_list_obsolete,
        "tools_clearlog" => routes::tools_clearlog,
        "tools_controllog" => routes::tools_controllog,
        _ => return None,
    })
}

/// Old CGI paths answered natively with images, a video stream or plain text.
fn raw_route(path: &str) -> Option<RawRoute> {
    let p = path.strip_prefix("/cgi-bin/")?;
    Some(match p {
        "snapshot_view" => routes::snapshot_view,
        "snapshot_get" => routes::snapshot_get,
        "webcam" => routes::webcam,
        "tools_ps" => routes::tools_ps,
        "tools_net" => routes::tools_net,
        "tools_ls" => routes::tools_ls,
        "tools_log" => routes::tools_log,
        "cmdlist" => routes::cmdlist,
        _ => return None,
    })
}

fn handle(stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let started = Instant::now();
    let req = http::read_request(&stream)?;
    if let Some(why) = routes::blocked(&req.path) {
        let body = format!("{{\"return\":\"1\",\"msg\":{}}}\n", http::json_str(&format!("Blocked by karotzd: this script {why}.")));
        return http::respond(&stream, 403, "text/plain", body.as_bytes());
    }
    if let Some(route) = json_route(&req.path) {
        if req.method != "GET" && req.method != "POST" {
            return http::respond(&stream, 405, "text/plain", b"GET or POST only\n");
        }
        let json = route(&req);
        return http::respond_json(&stream, &json);
    }
    if let Some(route) = raw_route(&req.path) {
        return route(&req, &stream);
    }
    match req.path.as_str() {
        "/" | "/ui" => http::respond(&stream, 200, "text/html; charset=utf-8", UI_HTML.as_bytes()),
        "/legacy" => {
            let mut s = &stream;
            s.write_all(b"HTTP/1.0 302 Found\r\nLocation: /index.html\r\nContent-Length: 0\r\n\r\n")
        }
        "/api/stop" => {
            karotz::killall(karotz::SOUND_PROCS);
            http::respond_json(&stream, "{\"return\":\"0\"}")
        }
        "/hello" => http::respond(&stream, 200, "text/plain", b"hello from rust on karotz\n"),
        "/api/info" => http::respond(&stream, 200, "application/json", info_json(started).as_bytes()),
        "/api/events" => events::poll(&stream, &req),
        "/api/events/stream" => {
            stream.set_write_timeout(Some(Duration::from_secs(30)))?;
            events::stream(&stream, &req)
        }
        _ => match UPSTREAM.get() {
            Some(up) => http::proxy(&stream, &req, up),
            None => http::respond(&stream, 404, "text/plain", b"not ported yet\n"),
        },
    }
}

fn first_line(path: &str) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().next().map(str::to_owned))
        .unwrap_or_default()
}

fn meminfo(key: &str) -> u64 {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// Same kind of data as the old `status` CGI, without forking ~30 processes.
fn info_json(started: Instant) -> String {
    let uptime = first_line("/proc/uptime");
    let uptime = uptime.split_whitespace().next().unwrap_or("0");
    format!(
        "{{\"server\":\"karotzd\",\"kernel\":{},\"uptime_s\":{},\"loadavg\":{},\
         \"mem_total_kb\":{},\"mem_free_kb\":{},\"handled_us\":{}}}\n",
        http::json_str(&first_line("/proc/sys/kernel/osrelease")),
        if uptime.is_empty() { "0" } else { uptime },
        http::json_str(&first_line("/proc/loadavg")),
        meminfo("MemTotal:"),
        meminfo("MemFree:"),
        started.elapsed().as_micros()
    )
}

// ---------------------------------------------------------------- D-Bus test

/// karotzd dbus-test [socket] [--led RRGGBB]
/// Connects, measures round trips to the bus daemon, and optionally sets the LED.
fn dbus_test(args: &[String]) -> i32 {
    if let Some(sock) = args.get(2).filter(|a| !a.starts_with("--")) {
        dbus::set_socket(sock);
    }
    let bus = ("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus");
    let t = Instant::now();
    match dbus::unique_name() {
        Ok(name) => println!("connected as {name} in {} us", t.elapsed().as_micros()),
        Err(e) => {
            println!("connect failed: {e:?}");
            return 1;
        }
    }
    // A string argument in, a string reply out: the daemon echoes our own name back.
    let me = dbus::unique_name().unwrap_or_default();
    let owner = dbus::call(bus.0, bus.1, bus.2, "GetNameOwner", &[dbus::Arg::S("org.freedesktop.DBus")]);
    let mine = dbus::call(bus.0, bus.1, bus.2, "GetNameOwner", &[dbus::Arg::S(&me)]);
    println!("GetNameOwner(org.freedesktop.DBus)={owner:?}  GetNameOwner({me})={mine:?}");
    let t = Instant::now();
    let mut id = None;
    for _ in 0..20 {
        id = dbus::call(bus.0, bus.1, bus.2, "GetId", &[]).ok().flatten();
    }
    println!("GetId x20: avg {} us, id={:?}", t.elapsed().as_micros() / 20, id);
    let t = Instant::now();
    let r = dbus::call("com.example.NoSuchService", "/x", "com.example.X", "Nope", &[dbus::Arg::S("a"), dbus::Arg::I(-1)]);
    println!("call to missing service ({} us): {r:?}", t.elapsed().as_micros());
    if args.iter().any(|a| a == "--emit") {
        // A made-up interface and member: nothing on the rabbit reacts to it
        // (dbus_watcher only acts on lines containing click / tag_in).
        let r = dbus::emit("/com/karotzd/Test", "com.karotzd.Test", "ping", &[dbus::Arg::S("hello"), dbus::Arg::I(42)]);
        println!("emitted com.karotzd.Test.ping: {r:?}");
    }
    if let Some(color) = flag_value(args, "--led").filter(|c| karotz::is_hex6(c)) {
        let uuid = first_line("/proc/sys/kernel/random/uuid");
        for _ in 0..2 {
            let t = Instant::now();
            let r = dbus::call("com.mindscape.karotz.Led", "/com/mindscape/karotz/Led",
                               "com.mindscape.karotz.KarotzInterface", "light",
                               &[dbus::Arg::S(&uuid), dbus::Arg::S(&color)]);
            println!("Led.light({color}) in {} us: {r:?}", t.elapsed().as_micros());
        }
    }
    0
}

// ---------------------------------------------------------------- self-test

fn check(name: &str, ok: bool, detail: String, fails: &mut u32) {
    println!("{}  {name}  {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        *fails += 1;
    }
}

fn http_get(addr: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(addr)?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: x\r\n\r\n")?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

fn body(resp: &str) -> &str {
    resp.split_once("\r\n\r\n").map(|(_, b)| b.trim_end()).unwrap_or("")
}

fn selftest() -> i32 {
    let mut fails = 0;
    println!("==== KAROTZD SELFTEST BEGIN ====");
    println!("kernel: {}", first_line("/proc/version"));

    // Threads + atomics (kuser cmpxchg helper on ARMv4T) + futex-based Mutex.
    let counter = Arc::new(AtomicUsize::new(0));
    let locked = Arc::new(Mutex::new(0u32));
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let (c, m) = (counter.clone(), locked.clone());
            thread::spawn(move || {
                for _ in 0..1000 {
                    c.fetch_add(1, Ordering::SeqCst);
                    *m.lock().unwrap() += 1;
                }
            })
        })
        .collect();
    let joined = hs.into_iter().all(|h| h.join().is_ok());
    let (a, m) = (counter.load(Ordering::SeqCst), *locked.lock().unwrap());
    check("threads+atomics+mutex", joined && a == 4000 && m == 4000, format!("atomic={a} mutex={m}"), &mut fails);

    // HashMap seeding (getrandom is missing; std falls back to /dev/urandom).
    let mut hm = HashMap::new();
    for i in 0..100 {
        hm.insert(i, i * 2);
    }
    check("hashmap", hm.get(&42) == Some(&84), format!("len={}", hm.len()), &mut fails);

    // Clocks and sleeping.
    let t = Instant::now();
    thread::sleep(Duration::from_millis(20));
    let ms = t.elapsed().as_millis();
    check("clock+sleep", (15..1000).contains(&ms), format!("slept {ms} ms"), &mut fails);

    // Files (statx missing; falls back to fstatat64).
    let p = env::temp_dir().join(format!("karotzd-selftest-{}", process::id()));
    let fs_ok = fs::write(&p, b"abc").is_ok()
        && fs::metadata(&p).map(|m| m.len() == 3).unwrap_or(false)
        && fs::read(&p).map(|d| d == b"abc").unwrap_or(false);
    let _ = fs::remove_file(&p);
    check("fs write/stat/read/remove", fs_ok && !p.exists(), p.display().to_string(), &mut fails);

    // Request parsing and input validation.
    let q = http::parse_query("color=FF00aa&x=a%20b+c&bad=%zz&empty");
    check("query decode", q["color"] == "FF00aa" && q["x"] == "a b c" && q["bad"] == "%zz" && q["empty"].is_empty(),
          format!("{q:?}"), &mut fails);
    let v = karotz::is_hex6("00ff00") && !karotz::is_hex6("00ff0") && !karotz::is_hex6("00ff0g")
        && karotz::is_sound_id("bip1") && !karotz::is_sound_id("../etc/passwd") && !karotz::is_sound_id(".x")
        && karotz::is_stream_url("http://radio/x.mp3") && !karotz::is_stream_url("-dumpfile")
        && !karotz::is_stream_url("http://a b");
    check("input validation", v, "hex colors, sound ids, stream urls".into(), &mut fails);

    // D-Bus codec: encode a signal, parse it back, decode its arguments.
    let raw = dbus::encode_signal(7, "/com/mindscape/karotz/Rfid", "com.mindscape.karotz.KarotzInterface", "tag_in",
        &[dbus::Arg::S("d0021a0353"), dbus::Arg::S("1"), dbus::Arg::I(0), dbus::Arg::I(3), dbus::Arg::I(0), dbus::Arg::I(-2)]);
    match dbus::read_msg(&mut &raw[..]) {
        Ok(m) => {
            let j = events::to_json(1, 5, &m);
            let good = m.mtype == 4 && m.member == "tag_in" && m.sig == "ssiiii"
                && dbus::decode_body(&m.sig, &m.body)[5] == dbus::Value::I(-2)
                && j.contains("\"source\":\"rfid\"") && j.contains("\"tag\":\"d0021a0353\"") && j.contains("\"tag_color\":-2");
            check("dbus signal codec", good, j, &mut fails);
        }
        Err(e) => check("dbus signal codec", false, e.to_string(), &mut fails),
    }

    // Routes, against a data dir that doesn't exist: no hardware is touched.
    karotz::init(karotz::Ctx { data: env::temp_dir().join("karotzd-no-such-dir"), readonly: true });
    match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => {
            let addr = l.local_addr().unwrap();
            let server = thread::spawn(move || {
                // 1 /hello + 50 /api/info + 11 route cases + 1 UI page
                for conn in l.incoming().take(63) {
                    if let Ok(s) = conn {
                        let _ = handle(s);
                    }
                }
            });
            let r = http_get(addr, "/hello").unwrap_or_else(|e| format!("error {e}"));
            check("http /hello", r.starts_with("HTTP/1.0 200") && body(&r) == "hello from rust on karotz",
                  r.lines().next().unwrap_or("").to_owned(), &mut fails);
            let t = Instant::now();
            let mut ok = 0;
            for _ in 0..50 {
                if http_get(addr, "/api/info").map(|r| r.contains("\"server\":\"karotzd\"")).unwrap_or(false) {
                    ok += 1;
                }
            }
            let us = t.elapsed().as_micros() / 50;
            check("http /api/info x50", ok == 50, format!("{ok}/50 ok, avg {us} us per request"), &mut fails);
            let cases = [
                ("/cgi-bin/leds?color=FF0000", "{\"return\":\"1\",\"msg\":\"Run directory not found.\"}"),
                ("/cgi-bin/ears?left=5", "{\"return\":\"1\",\"msg\":\"Missing mandatory parameters.\"}"),
                ("/cgi-bin/sound?id=a&url=b", "{\"return\":\"1\",\"msg\":\"You cannot use ID and URL parameters at the same time.\"}"),
                ("/cgi-bin/sound?id=..%2Fx", "{\"return\":\"1\",\"msg\":\"Unable to find sound : (invalid name)\"}"),
                ("/cgi-bin/sound?url=-dumpfile", "{\"return\":\"1\",\"msg\":\"url must start with http:// or https://\"}"),
                ("/cgi-bin/sound_list", "{\"sounds\":[],\"return\":\"0\"}"),
                ("/cgi-bin/rfid_info?tag=..%2Fx", "{\"return\":\"1\",\"msg\":\"Tag ID not found.\"}"),
                ("/cgi-bin/rfid_assign_url?tag=abc&url=file:///etc/passwd", "{\"return\":\"1\",\"msg\":\"url must start with http:// or https://\"}"),
                ("/cgi-bin/sound_control?cmd=run%20reboot", "{\"return\":\"1\",\"msg\":\"Unknown command.\"}"),
                ("/cgi-bin/sleep", "{\"return\":\"1\",\"msg\":\"karotzd is running read-only, so it won't put the rabbit to sleep (that writes to flash).\"}"),
                ("/cgi-bin/cmd?cmd=reboot", "{\"return\":\"1\",\"msg\":\"Blocked by karotzd: this script runs any shell command from the URL as root.\"}"),
            ];
            for (path, want) in cases {
                let r = http_get(addr, path).unwrap_or_else(|e| format!("error {e}"));
                check(&format!("route {path}"), body(&r) == want, body(&r).to_owned(), &mut fails);
            }
            let ui = http_get(addr, "/").unwrap_or_default();
            check("web UI at /", ui.starts_with("HTTP/1.0 200") && ui.contains("<title>Karotz</title>"),
                  format!("{} bytes", ui.len()), &mut fails);
            let md5 = util::md5_hex(b"The quick brown fox jumps over the lazy dog");
            let b64 = util::b64_decode(&util::b64_encode(b"curl 'x'\n")).unwrap_or_default();
            check("md5 + base64", md5 == "9e107d9d372bb6826bd81d3542a419d6" && b64 == b"curl 'x'\n", md5, &mut fails);
            let _ = server.join();
        }
        Err(e) => check("tcp bind", false, e.to_string(), &mut fails),
    }

    // Process spawn with piped output (pipe2 + clone/vfork + execve).
    let exe = env::current_exe().unwrap_or_else(|_| env::args().next().unwrap_or_default().into());
    match process::Command::new(&exe).arg("child").output() {
        Ok(o) => check("spawn child", o.status.success() && o.stdout == b"child-ok\n",
                       format!("{} -> {:?}", exe.display(), String::from_utf8_lossy(&o.stdout).trim()), &mut fails),
        Err(e) => check("spawn child", false, e.to_string(), &mut fails),
    }

    println!("RESULT: {} failure(s)", fails);
    println!("==== KAROTZD SELFTEST END ====");
    (fails != 0) as i32
}

// ---------------------------------------------------------------- PID 1 helpers (emulator only)

mod sys {
    use std::ffi::{c_char, c_int, c_ulong, c_void};
    extern "C" {
        fn mount(src: *const c_char, tgt: *const c_char, fst: *const c_char, fl: c_ulong, data: *const c_void) -> c_int;
        fn mkdir(path: *const c_char, mode: u32) -> c_int;
        fn socket(d: c_int, t: c_int, p: c_int) -> c_int;
        fn ioctl(fd: c_int, req: c_ulong, ...) -> c_int;
        fn close(fd: c_int) -> c_int;
        fn reboot(cmd: c_int) -> c_int;
        fn sync();
    }

    pub fn pid1_setup() {
        unsafe {
            mkdir(c"/proc".as_ptr(), 0o555);
            mount(c"proc".as_ptr(), c"/proc".as_ptr(), c"proc".as_ptr(), 0, std::ptr::null());
            mkdir(c"/tmp".as_ptr(), 0o1777);
            // ifconfig lo up: struct ifreq is 16 bytes of name, then a short of flags.
            let fd = socket(2, 2, 0);
            if fd >= 0 {
                let mut ifr = [0u8; 32];
                ifr[..2].copy_from_slice(b"lo");
                ioctl(fd, 0x8913, ifr.as_mut_ptr()); // SIOCGIFFLAGS
                let flags = u16::from_ne_bytes([ifr[16], ifr[17]]) | 0x1 | 0x40; // UP|RUNNING
                ifr[16..18].copy_from_slice(&flags.to_ne_bytes());
                ioctl(fd, 0x8914, ifr.as_mut_ptr()); // SIOCSIFFLAGS
                close(fd);
            }
        }
    }

    pub fn power_off() {
        unsafe {
            sync();
            reboot(0x01234567); // RESTART; QEMU's -no-reboot turns it into an exit.
        }
    }
}
